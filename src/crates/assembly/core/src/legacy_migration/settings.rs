use super::common::{
    backup_domain_dir, backup_file_once, read_bounded_json, read_optional_bounded_json,
    restore_unverified_file, stage_domain_dir,
};
use crate::service::config::manager::validate_current_config_value;
use crate::service::config::types::{AIModelConfig, AgentProfileConfig, GlobalConfig};
use openbitfun_legacy_migration::{
    atomic_write_json, DomainContext, DomainScan, LegacyDomainAdapter, LegacyMigrationError,
    LegacyMigrationResult, MigrationRoots,
};
use openbitfun_product_domains::legacy_migration::{
    ConflictResolution, FindingSeverity, MigrationConflict, MigrationDiagnostic, MigrationDomainId,
    MigrationDomainResult, MigrationDomainState, ScanFinding,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;

const SOURCE_SCHEMA: &str = "bitfun.config.v1";
const TARGET_SCHEMA: &str = "openbitfun.config.current";

pub(crate) struct SettingsAdapter;
pub(crate) struct CredentialsAdapter;

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StagedSettings {
    target_existed: bool,
    imported: u64,
    skipped: u64,
    conflicts: u64,
    config: GlobalConfig,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct CredentialManifest {
    target_existed: bool,
    model_ids: Vec<String>,
    voice_call: bool,
    unsupported_secret_fields: Vec<String>,
}

#[derive(Default)]
struct MergeOutcome {
    imported: u64,
    skipped: u64,
    conflicts: Vec<MigrationConflict>,
}

impl LegacyDomainAdapter for SettingsAdapter {
    fn domain(&self) -> MigrationDomainId {
        MigrationDomainId::Settings
    }

    fn scan(&self, roots: &MigrationRoots) -> LegacyMigrationResult<DomainScan> {
        let source = read_source_config(roots)?;
        let target = read_target_config(roots)?;
        let (_, outcome) = merge_settings(&source, target)?;
        let bytes = serde_json::to_vec(&source)
            .map_err(|error| LegacyMigrationError::InvalidRequest(error.to_string()))?;
        Ok(DomainScan {
            finding: ScanFinding {
                domain: self.domain(),
                code: "legacy_settings_supported".to_string(),
                severity: FindingSeverity::Info,
                entity_count: outcome.imported + outcome.skipped,
                logical_bytes: bytes.len() as u64,
                source_schema: Some(SOURCE_SCHEMA.to_string()),
                migratable: true,
                detail: "Supported legacy settings will be converted through the current configuration contract."
                    .to_string(),
            },
            conflicts: outcome.conflicts,
            target_schema: Some(TARGET_SCHEMA.to_string()),
            dependencies: Vec::new(),
        })
    }

    fn stage(&self, context: &DomainContext<'_>) -> LegacyMigrationResult<MigrationDomainResult> {
        let source = read_source_config(context.roots)?;
        let target_path = target_config_path(context.roots);
        let target_existed = target_path.exists();
        let target = read_target_config(context.roots)?;
        let (config, outcome) = merge_settings(&source, target)?;
        validate_current_config(&config, "staged legacy configuration")?;
        let staged = StagedSettings {
            target_existed,
            imported: outcome.imported,
            skipped: outcome.skipped,
            conflicts: outcome.conflicts.len() as u64,
            config,
        };
        atomic_write_json(&settings_stage_path(context), &staged)?;
        Ok(MigrationDomainResult {
            domain: self.domain(),
            state: MigrationDomainState::Staged,
            imported: staged.imported,
            skipped: staged.skipped,
            conflicts: staged.conflicts,
            ..MigrationDomainResult::default()
        })
    }

    fn validate_stage(&self, context: &DomainContext<'_>) -> LegacyMigrationResult<()> {
        let staged: StagedSettings =
            read_bounded_json(&context.layout.stage_root(), &settings_stage_path(context))?;
        validate_current_config(&staged.config, "staged legacy configuration")
    }

    fn commit(&self, context: &DomainContext<'_>) -> LegacyMigrationResult<()> {
        let staged: StagedSettings =
            read_bounded_json(&context.layout.stage_root(), &settings_stage_path(context))?;
        let target = target_config_path(context.roots);
        backup_file_once(&target, &settings_backup_path(context))?;
        atomic_write_json(&target, &staged.config)
    }

    fn validate_commit(&self, context: &DomainContext<'_>) -> LegacyMigrationResult<()> {
        let expected: StagedSettings =
            read_bounded_json(&context.layout.stage_root(), &settings_stage_path(context))?;
        let actual = read_target_config(context.roots)?;
        validate_current_config(&actual, "committed legacy configuration")?;
        let expected_value = serde_json::to_value(expected.config)
            .map_err(|error| LegacyMigrationError::InvalidRequest(error.to_string()))?;
        let actual_value = serde_json::to_value(actual)
            .map_err(|error| LegacyMigrationError::InvalidRequest(error.to_string()))?;
        if actual_value != expected_value {
            return Err(LegacyMigrationError::InvalidRequest(
                "committed configuration does not match the staged owner model".to_string(),
            ));
        }
        Ok(())
    }

    fn rollback_unverified(&self, context: &DomainContext<'_>) -> LegacyMigrationResult<()> {
        let staged = read_optional_bounded_json::<StagedSettings>(
            &context.layout.stage_root(),
            &settings_stage_path(context),
        )?;
        if let Some(staged) = staged {
            restore_unverified_file(
                &target_config_path(context.roots),
                &settings_backup_path(context),
                staged.target_existed,
            )?;
        }
        Ok(())
    }
}

impl LegacyDomainAdapter for CredentialsAdapter {
    fn domain(&self) -> MigrationDomainId {
        MigrationDomainId::Credentials
    }

    fn scan(&self, roots: &MigrationRoots) -> LegacyMigrationResult<DomainScan> {
        let source = read_source_config(roots)?;
        let manifest = credential_manifest(&source, target_config_path(roots).exists());
        let count = manifest.model_ids.len() as u64 + u64::from(manifest.voice_call);
        Ok(DomainScan {
            finding: ScanFinding {
                domain: self.domain(),
                code: if count == 0 {
                    "legacy_credentials_absent"
                } else {
                    "legacy_credentials_supported"
                }
                .to_string(),
                severity: if manifest.unsupported_secret_fields.is_empty() {
                    FindingSeverity::Info
                } else {
                    FindingSeverity::Warning
                },
                entity_count: count,
                logical_bytes: 0,
                source_schema: Some(SOURCE_SCHEMA.to_string()),
                migratable: true,
                detail: "Credentials are read only during commit and are never written to migration staging or reports."
                    .to_string(),
            },
            conflicts: Vec::new(),
            target_schema: Some(TARGET_SCHEMA.to_string()),
            dependencies: vec![MigrationDomainId::Settings],
        })
    }

    fn stage(&self, context: &DomainContext<'_>) -> LegacyMigrationResult<MigrationDomainResult> {
        let source = read_source_config(context.roots)?;
        let manifest = credential_manifest(&source, target_config_path(context.roots).exists());
        atomic_write_json(&credentials_stage_path(context), &manifest)?;
        let warnings = manifest
            .unsupported_secret_fields
            .iter()
            .map(|field| MigrationDiagnostic {
                code: "credential_requires_reauthentication".to_string(),
                severity: FindingSeverity::Warning,
                domain: Some(self.domain()),
                relative_path: None,
                message: format!("Credential metadata at {field} is not safely portable."),
                action: Some("Enter the credential again in OpenBitFun.".to_string()),
            })
            .collect::<Vec<_>>();
        Ok(MigrationDomainResult {
            domain: self.domain(),
            state: MigrationDomainState::Staged,
            imported: manifest.model_ids.len() as u64 + u64::from(manifest.voice_call),
            skipped: manifest.unsupported_secret_fields.len() as u64,
            warnings,
            requires_reauthentication: manifest.unsupported_secret_fields.clone(),
            ..MigrationDomainResult::default()
        })
    }

    fn validate_stage(&self, context: &DomainContext<'_>) -> LegacyMigrationResult<()> {
        let bytes = fs::read(credentials_stage_path(context)).map_err(|error| {
            LegacyMigrationError::InvalidRequest(format!("credential stage is unreadable: {error}"))
        })?;
        let manifest: CredentialManifest = serde_json::from_slice(&bytes).map_err(|error| {
            LegacyMigrationError::InvalidRequest(format!("credential stage is invalid: {error}"))
        })?;
        let source = read_source_config(context.roots)?;
        let mut secret_values = Vec::new();
        collect_secret_values(&source, &mut secret_values);
        for secret in secret_values.into_iter().filter(|secret| secret.len() >= 4) {
            if bytes
                .windows(secret.len())
                .any(|window| window == secret.as_bytes())
            {
                return Err(LegacyMigrationError::InvalidRequest(
                    "credential staging contains a prohibited secret value".to_string(),
                ));
            }
        }
        if manifest.model_ids.iter().any(|id| id.trim().is_empty()) {
            return Err(LegacyMigrationError::InvalidRequest(
                "credential staging contains an empty logical model id".to_string(),
            ));
        }
        Ok(())
    }

    fn commit(&self, context: &DomainContext<'_>) -> LegacyMigrationResult<()> {
        let source = read_source_config(context.roots)?;
        let target_path = target_config_path(context.roots);
        let mut target = read_target_config(context.roots)?;
        let manifest: CredentialManifest = read_bounded_json(
            &context.layout.stage_root(),
            &credentials_stage_path(context),
        )?;
        backup_file_once(&target_path, &credentials_backup_path(context))?;

        if let Some(models) = source.pointer("/ai/models").and_then(Value::as_array) {
            for source_model in models {
                let Some(id) = source_model.get("id").and_then(Value::as_str) else {
                    continue;
                };
                if !manifest.model_ids.iter().any(|candidate| candidate == id) {
                    continue;
                }
                let Some(secret) = source_model.get("api_key").and_then(Value::as_str) else {
                    continue;
                };
                if let Some(target_model) = target.ai.models.iter_mut().find(|model| model.id == id)
                {
                    if target_model.api_key.is_empty() {
                        target_model.api_key = secret.to_string();
                    }
                }
            }
        }
        if manifest.voice_call && target.app.voice_call.api_key.is_empty() {
            if let Some(secret) = source
                .pointer("/app/voice_call/api_key")
                .and_then(Value::as_str)
            {
                target.app.voice_call.api_key = secret.to_string();
            }
        }
        validate_current_config(&target, "credential migration target")?;
        atomic_write_json(&target_path, &target)
    }

    fn validate_commit(&self, context: &DomainContext<'_>) -> LegacyMigrationResult<()> {
        let source = read_source_config(context.roots)?;
        let target = read_target_config(context.roots)?;
        let manifest: CredentialManifest = read_bounded_json(
            &context.layout.stage_root(),
            &credentials_stage_path(context),
        )?;
        validate_current_config(&target, "committed credential configuration")?;
        for id in &manifest.model_ids {
            let source_secret = source
                .pointer("/ai/models")
                .and_then(Value::as_array)
                .and_then(|models| {
                    models
                        .iter()
                        .find(|model| model.get("id").and_then(Value::as_str) == Some(id.as_str()))
                })
                .and_then(|model| model.get("api_key"))
                .and_then(Value::as_str);
            let target_secret = target
                .ai
                .models
                .iter()
                .find(|model| model.id == *id)
                .map(|model| model.api_key.as_str());
            if source_secret.is_some_and(|secret| !secret.is_empty())
                && target_secret.is_none_or(str::is_empty)
            {
                return Err(LegacyMigrationError::InvalidRequest(format!(
                    "credential for model {id} was not committed"
                )));
            }
        }
        Ok(())
    }

    fn rollback_unverified(&self, context: &DomainContext<'_>) -> LegacyMigrationResult<()> {
        let manifest = read_optional_bounded_json::<CredentialManifest>(
            &context.layout.stage_root(),
            &credentials_stage_path(context),
        )?;
        if let Some(manifest) = manifest {
            restore_unverified_file(
                &target_config_path(context.roots),
                &credentials_backup_path(context),
                manifest.target_existed,
            )?;
        }
        Ok(())
    }
}

fn read_source_config(roots: &MigrationRoots) -> LegacyMigrationResult<Value> {
    read_bounded_json(&roots.legacy_user_root, &source_config_path(roots))
}

fn read_target_config(roots: &MigrationRoots) -> LegacyMigrationResult<GlobalConfig> {
    let path = target_config_path(roots);
    let value = read_optional_bounded_json::<Value>(&roots.target_user_root, &path)?;
    match value {
        Some(value) => {
            validate_current_config_value(&value, "legacy migration target configuration")
                .map_err(|error| LegacyMigrationError::InvalidRequest(error.to_string()))?;
            serde_json::from_value(value).map_err(|error| {
                LegacyMigrationError::InvalidRequest(format!(
                    "target configuration does not match the current owner model: {error}"
                ))
            })
        }
        None => Ok(GlobalConfig::default()),
    }
}

fn merge_settings(
    source: &Value,
    mut target: GlobalConfig,
) -> LegacyMigrationResult<(GlobalConfig, MergeOutcome)> {
    validate_source_version(source)?;
    let defaults = GlobalConfig::default();
    let mut outcome = MergeOutcome::default();

    merge_scalar(
        source.pointer("/app/language").and_then(Value::as_str),
        &mut target.app.language,
        &defaults.app.language,
        "app.language",
        &mut outcome,
    );
    merge_copy_scalar(
        source.pointer("/app/auto_update").and_then(Value::as_bool),
        &mut target.app.auto_update,
        defaults.app.auto_update,
        "app.auto_update",
        &mut outcome,
    );
    merge_copy_scalar(
        source.pointer("/app/telemetry").and_then(Value::as_bool),
        &mut target.app.telemetry,
        defaults.app.telemetry,
        "app.telemetry",
        &mut outcome,
    );
    if let Some(theme) = source
        .pointer("/appearance/theme_id")
        .and_then(Value::as_str)
    {
        let theme = canonical_product_id(theme);
        merge_scalar(
            Some(&theme),
            &mut target.appearance.selection,
            &defaults.appearance.selection,
            "appearance.selection",
            &mut outcome,
        );
    }

    if let Some(profiles) = source
        .pointer("/ai/agent_profiles")
        .and_then(Value::as_object)
    {
        for (profile_id, profile) in profiles {
            let enabled = profile
                .get("enabled_skills")
                .and_then(Value::as_array)
                .map(|items| {
                    items
                        .iter()
                        .filter_map(Value::as_str)
                        .map(canonical_product_id)
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            if enabled.is_empty() {
                continue;
            }
            let entry = target
                .ai
                .agent_profiles
                .entry(profile_id.clone())
                .or_insert_with(|| AgentProfileConfig {
                    profile_id: profile_id.clone(),
                    ..AgentProfileConfig::default()
                });
            if entry.enabled_user_skills.is_empty() {
                entry.enabled_user_skills = enabled;
                outcome.imported += 1;
            } else {
                record_target_wins(
                    &format!("ai.agent_profiles.{profile_id}.enabled_user_skills"),
                    &mut outcome,
                );
            }
        }
    }

    if let Some(models) = source.pointer("/ai/models").and_then(Value::as_array) {
        for raw in models {
            let Ok(mut model) = serde_json::from_value::<AIModelConfig>(raw.clone()) else {
                outcome.skipped += 1;
                continue;
            };
            if model.id.trim().is_empty() {
                outcome.skipped += 1;
                continue;
            }
            model.api_key.clear();
            model.custom_headers = None;
            model.custom_request_body = None;
            if target
                .ai
                .models
                .iter()
                .any(|existing| existing.id == model.id)
            {
                record_target_wins(&format!("ai.models.{}", model.id), &mut outcome);
            } else {
                target.ai.models.push(model);
                outcome.imported += 1;
            }
        }
    }

    target.product_id = defaults.product_id;
    target.schema_version = defaults.schema_version;
    target.version = defaults.version;
    target.last_modified = chrono::Utc::now();
    Ok((target, outcome))
}

fn credential_manifest(source: &Value, target_existed: bool) -> CredentialManifest {
    let model_ids = source
        .pointer("/ai/models")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|model| {
            model
                .get("api_key")
                .and_then(Value::as_str)
                .is_some_and(|secret| !secret.is_empty())
        })
        .filter_map(|model| model.get("id").and_then(Value::as_str))
        .filter(|id| !id.trim().is_empty())
        .map(str::to_string)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let voice_call = source
        .pointer("/app/voice_call/api_key")
        .and_then(Value::as_str)
        .is_some_and(|secret| !secret.is_empty());
    let mut unsupported_secret_fields = Vec::new();
    collect_unsupported_secret_fields(source, "", &mut unsupported_secret_fields);
    unsupported_secret_fields.retain(|path| {
        path != "app.voice_call.api_key"
            && !(path.starts_with("ai.models.") && path.ends_with(".api_key"))
    });
    unsupported_secret_fields.sort();
    unsupported_secret_fields.dedup();
    CredentialManifest {
        target_existed,
        model_ids,
        voice_call,
        unsupported_secret_fields,
    }
}

fn collect_unsupported_secret_fields(value: &Value, path: &str, output: &mut Vec<String>) {
    match value {
        Value::Object(fields) => {
            for (name, value) in fields {
                let next = if path.is_empty() {
                    name.to_string()
                } else {
                    format!("{path}.{name}")
                };
                let lower = name.to_ascii_lowercase();
                if matches!(lower.as_str(), "password" | "token" | "secret" | "api_key")
                    && value.as_str().is_some_and(|secret| !secret.is_empty())
                {
                    output.push(next.clone());
                }
                collect_unsupported_secret_fields(value, &next, output);
            }
        }
        Value::Array(items) => {
            for (index, value) in items.iter().enumerate() {
                collect_unsupported_secret_fields(value, &format!("{path}.{index}"), output);
            }
        }
        _ => {}
    }
}

fn collect_secret_values(value: &Value, output: &mut Vec<String>) {
    match value {
        Value::Object(fields) => {
            for (name, value) in fields {
                let lower = name.to_ascii_lowercase();
                if matches!(lower.as_str(), "password" | "token" | "secret" | "api_key") {
                    if let Some(secret) = value.as_str().filter(|secret| !secret.is_empty()) {
                        output.push(secret.to_string());
                    }
                }
                collect_secret_values(value, output);
            }
        }
        Value::Array(items) => {
            for value in items {
                collect_secret_values(value, output);
            }
        }
        _ => {}
    }
}

fn validate_source_version(source: &Value) -> LegacyMigrationResult<()> {
    let schema = source.get("schema_version").and_then(Value::as_u64);
    let version = source.get("version").and_then(Value::as_str);
    if schema != Some(1) || !version.is_some_and(|version| version.starts_with("0.")) {
        return Err(LegacyMigrationError::UnsupportedSource(format!(
            "expected BitFun config schema 1 from a 0.x release, found schema={schema:?}, version={version:?}"
        )));
    }
    Ok(())
}

fn validate_current_config(config: &GlobalConfig, context: &str) -> LegacyMigrationResult<()> {
    let value = serde_json::to_value(config)
        .map_err(|error| LegacyMigrationError::InvalidRequest(error.to_string()))?;
    validate_current_config_value(&value, context)
        .map_err(|error| LegacyMigrationError::InvalidRequest(error.to_string()))?;
    serde_json::from_value::<GlobalConfig>(value)
        .map(|_| ())
        .map_err(|error| LegacyMigrationError::InvalidRequest(error.to_string()))
}

fn merge_scalar(
    source: Option<&str>,
    target: &mut String,
    default: &str,
    path: &str,
    outcome: &mut MergeOutcome,
) {
    let Some(source) = source else { return };
    if target == default {
        *target = source.to_string();
        outcome.imported += 1;
    } else {
        record_target_wins(path, outcome);
    }
}

fn merge_copy_scalar<T: Copy + PartialEq>(
    source: Option<T>,
    target: &mut T,
    default: T,
    path: &str,
    outcome: &mut MergeOutcome,
) {
    let Some(source) = source else { return };
    if *target == default {
        *target = source;
        outcome.imported += 1;
    } else {
        record_target_wins(path, outcome);
    }
}

fn record_target_wins(path: &str, outcome: &mut MergeOutcome) {
    outcome.skipped += 1;
    outcome.conflicts.push(MigrationConflict {
        domain: MigrationDomainId::Settings,
        code: "target_setting_wins".to_string(),
        source_summary: format!("legacy setting {path}"),
        target_summary: format!("existing OpenBitFun setting {path}"),
        resolution: ConflictResolution::TargetWins,
    });
}

fn canonical_product_id(value: &str) -> String {
    value
        .replace("user::bitfun::", "user::openbitfun::")
        .replace("bitfun-", "openbitfun-")
}

fn source_config_path(roots: &MigrationRoots) -> PathBuf {
    roots.legacy_user_root.join("config").join("app.json")
}

fn target_config_path(roots: &MigrationRoots) -> PathBuf {
    roots.target_user_root.join("config").join("app.json")
}

fn settings_stage_path(context: &DomainContext<'_>) -> PathBuf {
    stage_domain_dir(context, "settings").join("app.json")
}

fn credentials_stage_path(context: &DomainContext<'_>) -> PathBuf {
    stage_domain_dir(context, "credentials").join("manifest.json")
}

fn settings_backup_path(context: &DomainContext<'_>) -> PathBuf {
    backup_domain_dir(context, "settings").join("app.json")
}

fn credentials_backup_path(context: &DomainContext<'_>) -> PathBuf {
    backup_domain_dir(context, "credentials").join("app.json")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::legacy_migration::adapters_for_groups;
    use openbitfun_legacy_migration::{
        probe_legacy_source, CancellationToken, MigrationEngine, NoCrashInjection, ProbeLimits,
    };
    use openbitfun_product_domains::legacy_migration::{MigrationGroupId, MigrationSelection};
    use sha2::{Digest, Sha256};
    use std::collections::BTreeSet;
    use std::path::Path;

    #[test]
    fn settings_and_credentials_convert_without_staging_secrets() {
        let temp = test_tempdir("settings-credentials");
        let roots = test_roots(temp.path());
        seed_source(&roots, true);
        let source_before = sha256(&source_config_path(&roots));
        let source = probe_legacy_source(&roots, ProbeLimits::default())
            .unwrap()
            .unwrap();
        let selection = MigrationSelection {
            groups: BTreeSet::from([MigrationGroupId::SettingsAndCredentials]),
        };
        let engine = MigrationEngine::new(roots.clone(), adapters_for_groups(&selection)).unwrap();
        let plan = engine
            .plan(&source, selection, &CancellationToken::default())
            .unwrap();
        let report = engine
            .execute(&plan, &CancellationToken::default(), &NoCrashInjection)
            .unwrap();

        let target = read_target_config(&roots).unwrap();
        assert_eq!(target.app.language, "zh-CN");
        assert!(!target.app.auto_update);
        assert_eq!(target.appearance.selection, "openbitfun-dark");
        assert_eq!(
            target.ai.agent_profiles["coding_shared"].enabled_user_skills,
            ["user::openbitfun::user-skill"]
        );
        assert_eq!(target.ai.models[0].api_key, "fixture-secret");
        assert_eq!(sha256(&source_config_path(&roots)), source_before);
        assert!(report.requires_reauthentication.is_empty());

        let stage = fs::read_dir(
            roots
                .migration_root()
                .join("runs")
                .join(&plan.run_id)
                .join("stage"),
        )
        .unwrap()
        .flat_map(|entry| walk_files(&entry.unwrap().path()))
        .flat_map(|path| fs::read(path).unwrap())
        .collect::<Vec<_>>();
        assert!(!String::from_utf8_lossy(&stage).contains("fixture-secret"));
    }

    #[test]
    fn explicit_target_values_win_and_are_reported() {
        let temp = test_tempdir("settings-target-wins");
        let roots = test_roots(temp.path());
        seed_source(&roots, true);
        let mut target = GlobalConfig::default();
        target.app.language = "en-US".to_string();
        target.ai.models.push(AIModelConfig {
            id: "legacy-model".to_string(),
            api_key: "target-secret".to_string(),
            ..AIModelConfig::default()
        });
        atomic_write_json(&target_config_path(&roots), &target).unwrap();
        let source = probe_legacy_source(&roots, ProbeLimits::default())
            .unwrap()
            .unwrap();
        let selection = MigrationSelection {
            groups: BTreeSet::from([MigrationGroupId::SettingsAndCredentials]),
        };
        let engine = MigrationEngine::new(roots.clone(), adapters_for_groups(&selection)).unwrap();
        let plan = engine
            .plan(&source, selection, &CancellationToken::default())
            .unwrap();
        assert!(plan
            .conflicts
            .iter()
            .any(|conflict| conflict.code == "target_setting_wins"));
        engine
            .execute(&plan, &CancellationToken::default(), &NoCrashInjection)
            .unwrap();
        let target = read_target_config(&roots).unwrap();
        assert_eq!(target.app.language, "en-US");
        assert_eq!(target.ai.models[0].api_key, "target-secret");
    }

    fn seed_source(roots: &MigrationRoots, with_secret: bool) {
        let source = serde_json::json!({
            "app": {"language": "zh-CN", "auto_update": false, "telemetry": false},
            "appearance": {"theme_id": "bitfun-dark"},
            "ai": {
                "agent_profiles": {"coding_shared": {"enabled_skills": ["user::bitfun::user-skill"]}},
                "models": [{
                    "id": "legacy-model",
                    "name": "Legacy model",
                    "provider": "openai",
                    "model_name": "legacy-model",
                    "base_url": "https://example.invalid/v1",
                    "api_key": if with_secret { "fixture-secret" } else { "" }
                }]
            },
            "schema_version": 1,
            "version": "0.2.19",
            "last_modified": 0
        });
        atomic_write_json(&source_config_path(roots), &source).unwrap();
    }

    fn test_roots(root: &Path) -> MigrationRoots {
        MigrationRoots {
            legacy_user_root: root.join("legacy-user"),
            legacy_home_root: root.join("legacy-home"),
            legacy_skills_root: root.join("legacy-skills"),
            legacy_ssh_root: root.join("legacy-ssh"),
            target_user_root: root.join("target-user"),
            target_home_root: root.join("target-home"),
            target_skills_root: root.join("target-skills"),
            target_ssh_root: root.join("target-ssh"),
        }
    }

    fn test_tempdir(label: &str) -> tempfile::TempDir {
        let root = std::env::var_os("OPENBITFUN_TEST_TMPDIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("E:/tmp"));
        fs::create_dir_all(&root).unwrap();
        tempfile::Builder::new()
            .prefix(&format!("openbitfun-migration-{label}-"))
            .tempdir_in(root)
            .unwrap()
    }

    fn sha256(path: &Path) -> Vec<u8> {
        Sha256::digest(fs::read(path).unwrap()).to_vec()
    }

    fn walk_files(root: &Path) -> Vec<PathBuf> {
        if root.is_file() {
            return vec![root.to_path_buf()];
        }
        fs::read_dir(root)
            .unwrap()
            .flat_map(|entry| walk_files(&entry.unwrap().path()))
            .collect()
    }
}
