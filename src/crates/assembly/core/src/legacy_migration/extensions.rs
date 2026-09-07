use super::common::{read_bounded_json, stage_domain_dir};
use openbitfun_agent_runtime::custom_agent::{
    custom_agent_read_markdown_str, custom_agent_save_markdown_file, CustomAgentDefinition,
    CustomAgentLevel,
};
use openbitfun_agent_runtime::skills::{SkillData, SkillLocation, OPENBITFUN_SYSTEM_SKILL_DIR};
use openbitfun_legacy_migration::{
    atomic_write_bytes, atomic_write_json, DomainContext, DomainScan, LegacyDomainAdapter,
    LegacyMigrationError, LegacyMigrationResult, MigrationRoots,
};
use openbitfun_product_domains::legacy_migration::{
    ConflictResolution, FindingSeverity, MigrationConflict, MigrationDiagnostic, MigrationDomainId,
    MigrationDomainResult, MigrationDomainState, ScanFinding,
};
use openbitfun_product_domains::miniapp::builtin::{BUILTIN_APPS, BUILTIN_INSTALL_MARKER};
use openbitfun_product_domains::miniapp::storage::{
    build_import_bundle_plan, MiniAppImportBundleWriteRequest, MiniAppStorageLayout, COMPILED_HTML,
    ESM_DEPS_JSON, INDEX_HTML, META_JSON, PACKAGE_JSON, REQUIRED_SOURCE_FILES, SOURCE_DIR,
    STORAGE_JSON, STYLE_CSS, UI_JS, WORKER_JS,
};
use openbitfun_product_domains::miniapp::types::MiniAppMeta;
use openbitfun_services_integrations::miniapp::storage::MiniAppStorage;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

const MAX_FILES_PER_EXTENSION: usize = 512;
const MAX_FILE_BYTES: u64 = 16 * 1024 * 1024;
const MAX_EXTENSION_BYTES: u64 = 64 * 1024 * 1024;
const SKILL_ALLOWED_DIRECTORIES: &[&str] =
    &["agents", "examples", "resources", "scripts", "templates"];
const MINIAPP_ALLOWED_FILES: &[&str] = &[
    META_JSON,
    INDEX_HTML,
    STYLE_CSS,
    UI_JS,
    WORKER_JS,
    ESM_DEPS_JSON,
    PACKAGE_JSON,
    STORAGE_JSON,
    BUILTIN_INSTALL_MARKER,
];

pub(crate) struct SkillsAdapter;
pub(crate) struct MiniappsAdapter;
pub(crate) struct AgentsAdapter;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ImportAction {
    Import,
    Remap,
    Duplicate,
    BuiltinStorage,
    TargetWins,
    Skip,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ImportEntry {
    source_id: String,
    target_id: String,
    action: ImportAction,
    content_hash: String,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct ImportManifest {
    entries: Vec<ImportEntry>,
    skipped_paths: Vec<String>,
}

#[derive(Debug)]
struct PlannedTree {
    source_id: String,
    target_id: String,
    action: ImportAction,
    content_hash: String,
    source_path: PathBuf,
    files: Vec<PathBuf>,
    skipped_paths: Vec<String>,
}

#[derive(Debug)]
struct PlannedAgent {
    source_id: String,
    target_id: String,
    action: ImportAction,
    content_hash: String,
    source_path: PathBuf,
    definition: CustomAgentDefinition,
}

impl LegacyDomainAdapter for SkillsAdapter {
    fn domain(&self) -> MigrationDomainId {
        MigrationDomainId::Skills
    }

    fn scan(&self, roots: &MigrationRoots) -> LegacyMigrationResult<DomainScan> {
        let planned = plan_skills(roots)?;
        Ok(scan_from_trees(
            self.domain(),
            "legacy_skills_supported",
            "agent-skill.v1",
            "openbitfun.agent-skill.current",
            &planned,
        ))
    }

    fn stage(&self, context: &DomainContext<'_>) -> LegacyMigrationResult<MigrationDomainResult> {
        let planned = plan_skills(context.roots)?;
        let output = stage_domain_dir(context, "skills").join("output");
        let mut manifest = ImportManifest::default();
        for item in &planned {
            if matches!(item.action, ImportAction::Import | ImportAction::Remap) {
                copy_declared_tree(
                    &item.source_path,
                    &output.join(&item.target_id),
                    &item.files,
                )?;
            }
            manifest.entries.push(import_entry(item));
            manifest.skipped_paths.extend(item.skipped_paths.clone());
        }
        atomic_write_json(&skills_manifest_path(context), &manifest)?;
        result_from_manifest(self.domain(), &manifest)
    }

    fn validate_stage(&self, context: &DomainContext<'_>) -> LegacyMigrationResult<()> {
        let manifest = read_manifest(context, "skills")?;
        for entry in imported_entries(&manifest) {
            let root = stage_domain_dir(context, "skills")
                .join("output")
                .join(&entry.target_id);
            let skill_path = root.join("SKILL.md");
            let content =
                fs::read_to_string(&skill_path).map_err(|error| io(&skill_path, error))?;
            SkillData::from_markdown(
                skill_path.to_string_lossy().to_string(),
                &content,
                SkillLocation::User,
                false,
            )
            .map_err(|error| {
                LegacyMigrationError::InvalidRequest(format!(
                    "staged Skill {} failed current owner parsing: {error}",
                    entry.target_id
                ))
            })?;
            require_hash(&root, &entry.content_hash)?;
        }
        Ok(())
    }

    fn commit(&self, context: &DomainContext<'_>) -> LegacyMigrationResult<()> {
        let manifest = read_manifest(context, "skills")?;
        let target_root = &context.roots.target_skills_root;
        for entry in imported_entries(&manifest) {
            let staged = stage_domain_dir(context, "skills")
                .join("output")
                .join(&entry.target_id);
            install_directory_idempotent(
                &staged,
                &target_root.join(&entry.target_id),
                &entry.content_hash,
                &context.plan.run_id,
            )?;
        }
        Ok(())
    }

    fn validate_commit(&self, context: &DomainContext<'_>) -> LegacyMigrationResult<()> {
        let manifest = read_manifest(context, "skills")?;
        for entry in imported_entries(&manifest) {
            let root = context.roots.target_skills_root.join(&entry.target_id);
            require_hash(&root, &entry.content_hash)?;
            let path = root.join("SKILL.md");
            let content = fs::read_to_string(&path).map_err(|error| io(&path, error))?;
            SkillData::from_markdown(
                path.to_string_lossy().to_string(),
                &content,
                SkillLocation::User,
                false,
            )
            .map_err(|error| {
                LegacyMigrationError::InvalidRequest(format!(
                    "committed Skill {} failed current owner parsing: {error}",
                    entry.target_id
                ))
            })?;
        }
        Ok(())
    }

    fn rollback_unverified(&self, context: &DomainContext<'_>) -> LegacyMigrationResult<()> {
        rollback_imported_directories(context, "skills", &context.roots.target_skills_root)
    }
}

impl LegacyDomainAdapter for MiniappsAdapter {
    fn domain(&self) -> MigrationDomainId {
        MigrationDomainId::Miniapps
    }

    fn scan(&self, roots: &MigrationRoots) -> LegacyMigrationResult<DomainScan> {
        let planned = plan_miniapps(roots)?;
        Ok(scan_from_trees(
            self.domain(),
            "legacy_miniapps_supported",
            "miniapp.flat.v1",
            "openbitfun.miniapp.current",
            &planned,
        ))
    }

    fn stage(&self, context: &DomainContext<'_>) -> LegacyMigrationResult<MigrationDomainResult> {
        let planned = plan_miniapps(context.roots)?;
        let domain_root = stage_domain_dir(context, "miniapps");
        let import_inputs = domain_root.join("import-inputs");
        let output = domain_root.join("output");
        let mut manifest = ImportManifest::default();
        for item in &planned {
            match item.action {
                ImportAction::Import | ImportAction::Remap => {
                    let input = import_inputs.join(&item.target_id);
                    prepare_miniapp_import_input(item, &input)?;
                    let meta = fs::read_to_string(input.join(META_JSON))
                        .map_err(|error| io(&input.join(META_JSON), error))?;
                    let plan =
                        build_import_bundle_plan(&item.target_id, &meta, 0).map_err(|error| {
                            LegacyMigrationError::InvalidRequest(format!(
                                "legacy MiniApp {} failed current owner conversion: {error}",
                                item.source_id
                            ))
                        })?;
                    MiniAppStorage::new(output.clone())
                        .write_import_bundle_offline(MiniAppImportBundleWriteRequest {
                            source_path: input,
                            app_id: item.target_id.clone(),
                            meta_json: plan.meta_json,
                            esm_dependencies_json: plan.esm_dependencies_json,
                            package_json: plan.package_json,
                            storage_json: plan.storage_json,
                            compiled_html: plan.compiled_html,
                        })
                        .map_err(|error| LegacyMigrationError::InvalidRequest(error.to_string()))?;
                }
                ImportAction::BuiltinStorage => {
                    let source = item.source_path.join(STORAGE_JSON);
                    let value: serde_json::Value = read_bounded_json(&item.source_path, &source)?;
                    atomic_write_json(
                        &domain_root
                            .join("builtin-storage")
                            .join(&item.target_id)
                            .join(STORAGE_JSON),
                        &value,
                    )?;
                }
                ImportAction::Duplicate | ImportAction::TargetWins | ImportAction::Skip => {}
            }
            manifest.entries.push(import_entry(item));
            manifest.skipped_paths.extend(item.skipped_paths.clone());
        }
        atomic_write_json(&miniapps_manifest_path(context), &manifest)?;
        result_from_manifest(self.domain(), &manifest)
    }

    fn validate_stage(&self, context: &DomainContext<'_>) -> LegacyMigrationResult<()> {
        let manifest = read_manifest(context, "miniapps")?;
        let storage = MiniAppStorage::new(PathBuf::new());
        for entry in imported_entries(&manifest) {
            let root = stage_domain_dir(context, "miniapps")
                .join("output")
                .join(&entry.target_id);
            let meta = storage
                .read_import_meta_json_offline(&root)
                .map_err(|error| LegacyMigrationError::InvalidRequest(error.to_string()))?;
            let parsed: MiniAppMeta = serde_json::from_str(&meta).map_err(|error| {
                LegacyMigrationError::InvalidRequest(format!(
                    "staged MiniApp {} failed current owner parsing: {error}",
                    entry.target_id
                ))
            })?;
            if parsed.id != entry.target_id {
                return Err(LegacyMigrationError::InvalidRequest(format!(
                    "staged MiniApp id mismatch: expected {}, found {}",
                    entry.target_id, parsed.id
                )));
            }
        }
        for entry in manifest
            .entries
            .iter()
            .filter(|entry| entry.action == ImportAction::BuiltinStorage)
        {
            let path = stage_domain_dir(context, "miniapps")
                .join("builtin-storage")
                .join(&entry.target_id)
                .join(STORAGE_JSON);
            let _: serde_json::Value = read_bounded_json(&context.layout.stage_root(), &path)?;
        }
        Ok(())
    }

    fn commit(&self, context: &DomainContext<'_>) -> LegacyMigrationResult<()> {
        let manifest = read_manifest(context, "miniapps")?;
        let target_root = target_miniapps_root(context.roots);
        for entry in imported_entries(&manifest) {
            let staged = stage_domain_dir(context, "miniapps")
                .join("output")
                .join(&entry.target_id);
            let staged_hash = hash_declared_current_miniapp(&staged)?;
            install_directory_idempotent(
                &staged,
                &target_root.join(&entry.target_id),
                &staged_hash,
                &context.plan.run_id,
            )?;
        }
        for entry in manifest
            .entries
            .iter()
            .filter(|entry| entry.action == ImportAction::BuiltinStorage)
        {
            let staged = stage_domain_dir(context, "miniapps")
                .join("builtin-storage")
                .join(&entry.target_id)
                .join(STORAGE_JSON);
            let target = target_root.join(&entry.target_id).join(STORAGE_JSON);
            if target.exists() {
                if hash_file(&target)? != hash_file(&staged)? {
                    return Err(LegacyMigrationError::InvalidRequest(format!(
                        "target MiniApp storage changed after planning: {}",
                        target.display()
                    )));
                }
            } else {
                let bytes = fs::read(&staged).map_err(|error| io(&staged, error))?;
                atomic_write_bytes(&target, &bytes)?;
            }
        }
        Ok(())
    }

    fn validate_commit(&self, context: &DomainContext<'_>) -> LegacyMigrationResult<()> {
        let manifest = read_manifest(context, "miniapps")?;
        let target_root = target_miniapps_root(context.roots);
        let storage = MiniAppStorage::new(PathBuf::new());
        for entry in imported_entries(&manifest) {
            let root = target_root.join(&entry.target_id);
            let meta = storage
                .read_import_meta_json_offline(&root)
                .map_err(|error| LegacyMigrationError::InvalidRequest(error.to_string()))?;
            let parsed: MiniAppMeta = serde_json::from_str(&meta).map_err(|error| {
                LegacyMigrationError::InvalidRequest(format!(
                    "committed MiniApp {} failed current owner parsing: {error}",
                    entry.target_id
                ))
            })?;
            if parsed.id != entry.target_id {
                return Err(LegacyMigrationError::InvalidRequest(format!(
                    "committed MiniApp id mismatch for {}",
                    entry.target_id
                )));
            }
        }
        for entry in manifest
            .entries
            .iter()
            .filter(|entry| entry.action == ImportAction::BuiltinStorage)
        {
            let path = target_root.join(&entry.target_id).join(STORAGE_JSON);
            let _: serde_json::Value = read_bounded_json(&target_root, &path)?;
        }
        Ok(())
    }

    fn rollback_unverified(&self, context: &DomainContext<'_>) -> LegacyMigrationResult<()> {
        rollback_imported_directories(context, "miniapps", &target_miniapps_root(context.roots))?;
        rollback_builtin_storage(context)
    }
}

impl LegacyDomainAdapter for AgentsAdapter {
    fn domain(&self) -> MigrationDomainId {
        MigrationDomainId::Agents
    }

    fn scan(&self, roots: &MigrationRoots) -> LegacyMigrationResult<DomainScan> {
        let planned = plan_agents(roots)?;
        let conflicts = planned
            .iter()
            .filter_map(|item| {
                conflict_for_action(self.domain(), &item.source_id, &item.target_id, item.action)
            })
            .collect();
        Ok(DomainScan {
            finding: ScanFinding {
                domain: self.domain(),
                code: "legacy_agents_supported".to_string(),
                severity: FindingSeverity::Info,
                entity_count: planned.len() as u64,
                logical_bytes: planned
                    .iter()
                    .map(|item| fs::metadata(&item.source_path).map_or(0, |meta| meta.len()))
                    .sum(),
                source_schema: Some("custom-agent.v1".to_string()),
                migratable: true,
                detail:
                    "Legacy user Agent definitions are parsed by the current owner before import."
                        .to_string(),
            },
            conflicts,
            target_schema: Some("openbitfun.custom-agent.current".to_string()),
            dependencies: vec![MigrationDomainId::Skills, MigrationDomainId::Miniapps],
        })
    }

    fn stage(&self, context: &DomainContext<'_>) -> LegacyMigrationResult<MigrationDomainResult> {
        let planned = plan_agents(context.roots)?;
        let output = stage_domain_dir(context, "agents").join("output");
        let mut manifest = ImportManifest::default();
        for item in planned {
            if matches!(item.action, ImportAction::Import | ImportAction::Remap) {
                let path = output.join(format!("{}.md", safe_component(&item.target_id)));
                if let Some(parent) = path.parent() {
                    fs::create_dir_all(parent).map_err(|error| io(parent, error))?;
                }
                custom_agent_save_markdown_file(&path, &item.definition).map_err(|error| {
                    LegacyMigrationError::InvalidRequest(format!(
                        "failed to stage Agent {} through the current owner: {error}",
                        item.target_id
                    ))
                })?;
                let content = fs::read(&path).map_err(|error| io(&path, error))?;
                manifest.entries.push(ImportEntry {
                    source_id: item.source_id,
                    target_id: item.target_id,
                    action: item.action,
                    content_hash: hash_bytes(&content),
                });
            } else {
                manifest.entries.push(ImportEntry {
                    source_id: item.source_id,
                    target_id: item.target_id,
                    action: item.action,
                    content_hash: item.content_hash,
                });
            }
        }
        atomic_write_json(&agents_manifest_path(context), &manifest)?;
        result_from_manifest(self.domain(), &manifest)
    }

    fn validate_stage(&self, context: &DomainContext<'_>) -> LegacyMigrationResult<()> {
        validate_agent_manifest(context, true)
    }

    fn commit(&self, context: &DomainContext<'_>) -> LegacyMigrationResult<()> {
        let manifest = read_manifest(context, "agents")?;
        let target_root = context.roots.target_user_root.join("agents");
        for entry in imported_entries(&manifest) {
            let name = format!("{}.md", safe_component(&entry.target_id));
            let staged = stage_domain_dir(context, "agents")
                .join("output")
                .join(&name);
            let target = target_root.join(&name);
            install_file_idempotent(&staged, &target, &entry.content_hash)?;
        }
        Ok(())
    }

    fn validate_commit(&self, context: &DomainContext<'_>) -> LegacyMigrationResult<()> {
        validate_agent_manifest(context, false)
    }

    fn rollback_unverified(&self, context: &DomainContext<'_>) -> LegacyMigrationResult<()> {
        let manifest = read_manifest(context, "agents")?;
        let target_root = context.roots.target_user_root.join("agents");
        for entry in imported_entries(&manifest) {
            let target = target_root.join(format!("{}.md", safe_component(&entry.target_id)));
            if target.exists() && hash_file(&target)? == entry.content_hash {
                fs::remove_file(&target).map_err(|error| io(&target, error))?;
            }
        }
        Ok(())
    }
}

fn plan_skills(roots: &MigrationRoots) -> LegacyMigrationResult<Vec<PlannedTree>> {
    let source_root = legacy_skills_root(roots);
    let target_root = &roots.target_skills_root;
    let mut planned = Vec::new();
    for source in direct_child_directories(&source_root)? {
        let source_id = file_name(&source)?;
        if source_id == OPENBITFUN_SYSTEM_SKILL_DIR {
            continue;
        }
        let skill_file = source.join("SKILL.md");
        let content = fs::read_to_string(&skill_file).map_err(|error| io(&skill_file, error))?;
        SkillData::from_markdown(
            skill_file.to_string_lossy().to_string(),
            &content,
            SkillLocation::User,
            false,
        )
        .map_err(|error| {
            LegacyMigrationError::InvalidRequest(format!(
                "legacy Skill {source_id} failed current owner parsing: {error}"
            ))
        })?;
        let (files, skipped_paths) = declared_skill_files(&source)?;
        let content_hash = hash_file_set(&source, &files)?;
        let target = target_root.join(&source_id);
        let (target_id, action) = resolve_directory_conflict(&source_id, &content_hash, &target)?;
        planned.push(PlannedTree {
            source_id,
            target_id,
            action,
            content_hash,
            source_path: source,
            files,
            skipped_paths,
        });
    }
    Ok(planned)
}

fn plan_miniapps(roots: &MigrationRoots) -> LegacyMigrationResult<Vec<PlannedTree>> {
    let source_root = roots.legacy_user_root.join("data").join("miniapps");
    let target_root = target_miniapps_root(roots);
    let builtin_ids = BUILTIN_APPS
        .iter()
        .map(|app| app.id)
        .collect::<BTreeSet<_>>();
    let mut planned = Vec::new();
    for source in direct_child_directories(&source_root)? {
        let directory_id = file_name(&source)?;
        let meta_path = source.join(META_JSON);
        let raw_meta: serde_json::Value = read_bounded_json(&source, &meta_path)?;
        let declared_id = raw_meta
            .get("id")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default();
        let source_id = if declared_id.trim().is_empty() {
            directory_id.clone()
        } else {
            declared_id.to_string()
        };
        let current_builtin_id = if builtin_ids.contains(source_id.as_str()) {
            Some(source_id.clone())
        } else if builtin_ids.contains(directory_id.as_str()) {
            Some(directory_id.clone())
        } else {
            None
        };
        let is_builtin =
            current_builtin_id.is_some() || source.join(BUILTIN_INSTALL_MARKER).exists();
        let (files, skipped_paths) = declared_flat_files(&source, MINIAPP_ALLOWED_FILES)?;
        let content_hash = hash_file_set(&source, &files)?;
        if is_builtin {
            let target_id = current_builtin_id
                .clone()
                .unwrap_or_else(|| remapped_id(&source_id, &content_hash));
            let target_storage = target_root.join(&target_id).join(STORAGE_JSON);
            planned.push(PlannedTree {
                source_id,
                target_id,
                action: if current_builtin_id.is_none() || !source.join(STORAGE_JSON).exists() {
                    ImportAction::Skip
                } else if target_storage.exists() {
                    ImportAction::TargetWins
                } else {
                    ImportAction::BuiltinStorage
                },
                content_hash,
                source_path: source,
                files,
                skipped_paths,
            });
            continue;
        }
        build_import_bundle_plan(
            &source_id,
            &fs::read_to_string(&meta_path).map_err(|error| io(&meta_path, error))?,
            0,
        )
        .map_err(|error| {
            LegacyMigrationError::InvalidRequest(format!(
                "legacy MiniApp {source_id} failed current owner conversion: {error}"
            ))
        })?;
        let (target_id, action) = if is_safe_component(&source_id) {
            resolve_miniapp_conflict(&source, &source_id, &content_hash, &target_root)?
        } else {
            resolve_remapped_miniapp_conflict(&source, &source_id, &content_hash, &target_root)?
        };
        planned.push(PlannedTree {
            source_id,
            target_id,
            action,
            content_hash,
            source_path: source,
            files,
            skipped_paths,
        });
    }
    Ok(planned)
}

fn plan_agents(roots: &MigrationRoots) -> LegacyMigrationResult<Vec<PlannedAgent>> {
    let source_root = roots.legacy_user_root.join("agents");
    let target_root = roots.target_user_root.join("agents");
    let mut target_by_id = BTreeMap::new();
    for path in direct_markdown_files(&target_root)? {
        let content = fs::read_to_string(&path).map_err(|error| io(&path, error))?;
        if let Ok(parsed) = custom_agent_read_markdown_str(&content, CustomAgentLevel::User) {
            target_by_id.insert(parsed.definition.id.to_ascii_lowercase(), parsed.definition);
        }
    }
    let mut planned = Vec::new();
    for source_path in direct_markdown_files(&source_root)? {
        let content = fs::read_to_string(&source_path).map_err(|error| io(&source_path, error))?;
        let parsed =
            custom_agent_read_markdown_str(&content, CustomAgentLevel::User).map_err(|error| {
                LegacyMigrationError::InvalidRequest(format!(
                    "legacy Agent {} failed current owner parsing: {error}",
                    source_path.display()
                ))
            })?;
        let source_id = parsed.definition.id.clone();
        let content_hash = hash_bytes(content.as_bytes());
        let (target_id, action, mut definition) =
            match target_by_id.get(&source_id.to_ascii_lowercase()) {
                Some(existing) if existing == &parsed.definition => (
                    source_id.clone(),
                    ImportAction::Duplicate,
                    parsed.definition,
                ),
                Some(_) => {
                    let target_id = remapped_id(&source_id, &content_hash);
                    let mut definition = parsed.definition;
                    definition.id = target_id.clone();
                    definition.name = format!("{} (from legacy product)", definition.name);
                    (target_id, ImportAction::Remap, definition)
                }
                None => (source_id.clone(), ImportAction::Import, parsed.definition),
            };
        definition.level = CustomAgentLevel::User;
        planned.push(PlannedAgent {
            source_id,
            target_id,
            action,
            content_hash,
            source_path,
            definition,
        });
    }
    Ok(planned)
}

fn prepare_miniapp_import_input(item: &PlannedTree, input: &Path) -> LegacyMigrationResult<()> {
    fs::create_dir_all(input.join(SOURCE_DIR)).map_err(|error| io(input, error))?;
    let meta = fs::read(&item.source_path.join(META_JSON))
        .map_err(|error| io(&item.source_path.join(META_JSON), error))?;
    atomic_write_bytes(&input.join(META_JSON), &meta)?;
    for name in REQUIRED_SOURCE_FILES {
        let source = item.source_path.join(name);
        let target = input.join(SOURCE_DIR).join(name);
        if source.exists() {
            let bytes = fs::read(&source).map_err(|error| io(&source, error))?;
            atomic_write_bytes(&target, &bytes)?;
        } else {
            atomic_write_bytes(&target, b"")?;
        }
    }
    for name in [ESM_DEPS_JSON, PACKAGE_JSON, STORAGE_JSON] {
        let source = item.source_path.join(name);
        if source.exists() {
            let bytes = fs::read(&source).map_err(|error| io(&source, error))?;
            let target = if name == ESM_DEPS_JSON {
                input.join(SOURCE_DIR).join(name)
            } else {
                input.join(name)
            };
            atomic_write_bytes(&target, &bytes)?;
        }
    }
    Ok(())
}

fn validate_agent_manifest(context: &DomainContext<'_>, staged: bool) -> LegacyMigrationResult<()> {
    let manifest = read_manifest(context, "agents")?;
    let root = if staged {
        stage_domain_dir(context, "agents").join("output")
    } else {
        context.roots.target_user_root.join("agents")
    };
    for entry in imported_entries(&manifest) {
        let path = root.join(format!("{}.md", safe_component(&entry.target_id)));
        let content = fs::read_to_string(&path).map_err(|error| io(&path, error))?;
        let parsed =
            custom_agent_read_markdown_str(&content, CustomAgentLevel::User).map_err(|error| {
                LegacyMigrationError::InvalidRequest(format!(
                    "Agent {} failed current owner parsing: {error}",
                    entry.target_id
                ))
            })?;
        if parsed.definition.id != entry.target_id
            || hash_bytes(content.as_bytes()) != entry.content_hash
        {
            return Err(LegacyMigrationError::InvalidRequest(format!(
                "Agent {} does not match its staged identity",
                entry.target_id
            )));
        }
    }
    Ok(())
}

fn scan_from_trees(
    domain: MigrationDomainId,
    code: &str,
    source_schema: &str,
    target_schema: &str,
    planned: &[PlannedTree],
) -> DomainScan {
    DomainScan {
        finding: ScanFinding {
            domain,
            code: code.to_string(),
            severity: FindingSeverity::Info,
            entity_count: planned.len() as u64,
            logical_bytes: planned
                .iter()
                .map(|item| {
                    item.files
                        .iter()
                        .filter_map(|path| fs::metadata(path).ok())
                        .map(|metadata| metadata.len())
                        .sum::<u64>()
                })
                .sum(),
            source_schema: Some(source_schema.to_string()),
            migratable: true,
            detail:
                "Only owner-approved files are staged; imported executable content remains inert."
                    .to_string(),
        },
        conflicts: planned
            .iter()
            .filter_map(|item| {
                conflict_for_action(domain, &item.source_id, &item.target_id, item.action)
            })
            .collect(),
        target_schema: Some(target_schema.to_string()),
        dependencies: match domain {
            MigrationDomainId::Miniapps => vec![MigrationDomainId::Skills],
            _ => Vec::new(),
        },
    }
}

fn conflict_for_action(
    domain: MigrationDomainId,
    source_id: &str,
    target_id: &str,
    action: ImportAction,
) -> Option<MigrationConflict> {
    let (code, resolution) = match action {
        ImportAction::Remap => ("extension_id_remapped", ConflictResolution::SourceRemapped),
        ImportAction::Duplicate => (
            "extension_duplicate_skipped",
            ConflictResolution::DuplicateSkipped,
        ),
        ImportAction::BuiltinStorage => {
            ("builtin_code_excluded", ConflictResolution::SourceImported)
        }
        ImportAction::TargetWins => (
            "builtin_storage_target_preserved",
            ConflictResolution::TargetWins,
        ),
        ImportAction::Skip => ("extension_item_skipped", ConflictResolution::ItemSkipped),
        ImportAction::Import => return None,
    };
    Some(MigrationConflict {
        domain,
        code: code.to_string(),
        source_summary: format!("legacy extension id {source_id}"),
        target_summary: format!("OpenBitFun extension id {target_id}"),
        resolution,
    })
}

fn result_from_manifest(
    domain: MigrationDomainId,
    manifest: &ImportManifest,
) -> LegacyMigrationResult<MigrationDomainResult> {
    let imported = manifest
        .entries
        .iter()
        .filter(|entry| {
            matches!(
                entry.action,
                ImportAction::Import | ImportAction::Remap | ImportAction::BuiltinStorage
            )
        })
        .count() as u64;
    let skipped = manifest
        .entries
        .iter()
        .filter(|entry| {
            matches!(
                entry.action,
                ImportAction::Duplicate | ImportAction::TargetWins | ImportAction::Skip
            )
        })
        .count() as u64
        + manifest.skipped_paths.len() as u64;
    let conflicts = manifest
        .entries
        .iter()
        .filter(|entry| entry.action != ImportAction::Import)
        .count() as u64;
    let warnings = manifest
        .skipped_paths
        .iter()
        .map(|path| MigrationDiagnostic {
            code: "extension_path_not_declared".to_string(),
            severity: FindingSeverity::Warning,
            domain: Some(domain),
            relative_path: Some(path.clone()),
            message: "The path is outside the current owner's import contract.".to_string(),
            action: Some("Review the source extension manually.".to_string()),
        })
        .collect();
    Ok(MigrationDomainResult {
        domain,
        state: MigrationDomainState::Staged,
        imported,
        skipped,
        conflicts,
        warnings,
        ..MigrationDomainResult::default()
    })
}

fn import_entry(item: &PlannedTree) -> ImportEntry {
    ImportEntry {
        source_id: item.source_id.clone(),
        target_id: item.target_id.clone(),
        action: item.action,
        content_hash: item.content_hash.clone(),
    }
}

fn imported_entries(manifest: &ImportManifest) -> impl Iterator<Item = &ImportEntry> {
    manifest
        .entries
        .iter()
        .filter(|entry| matches!(entry.action, ImportAction::Import | ImportAction::Remap))
}

fn read_manifest(context: &DomainContext<'_>, name: &str) -> LegacyMigrationResult<ImportManifest> {
    read_bounded_json(
        &context.layout.stage_root(),
        &stage_domain_dir(context, name).join("manifest.json"),
    )
}

fn skills_manifest_path(context: &DomainContext<'_>) -> PathBuf {
    stage_domain_dir(context, "skills").join("manifest.json")
}

fn miniapps_manifest_path(context: &DomainContext<'_>) -> PathBuf {
    stage_domain_dir(context, "miniapps").join("manifest.json")
}

fn agents_manifest_path(context: &DomainContext<'_>) -> PathBuf {
    stage_domain_dir(context, "agents").join("manifest.json")
}

fn legacy_skills_root(roots: &MigrationRoots) -> PathBuf {
    if roots.legacy_skills_root.exists() {
        roots.legacy_skills_root.clone()
    } else {
        roots.legacy_user_root.join("skills")
    }
}

fn target_miniapps_root(roots: &MigrationRoots) -> PathBuf {
    roots.target_user_root.join("data").join("miniapps")
}

fn direct_child_directories(root: &Path) -> LegacyMigrationResult<Vec<PathBuf>> {
    if !root.exists() {
        return Ok(Vec::new());
    }
    reject_link(root)?;
    let mut output = Vec::new();
    for entry in fs::read_dir(root).map_err(|error| io(root, error))? {
        let entry = entry.map_err(|error| io(root, error))?;
        let path = entry.path();
        reject_link(&path)?;
        if entry
            .file_type()
            .map_err(|error| io(&path, error))?
            .is_dir()
        {
            output.push(path);
        }
    }
    output.sort();
    Ok(output)
}

fn direct_markdown_files(root: &Path) -> LegacyMigrationResult<Vec<PathBuf>> {
    if !root.exists() {
        return Ok(Vec::new());
    }
    reject_link(root)?;
    let mut output = Vec::new();
    for entry in fs::read_dir(root).map_err(|error| io(root, error))? {
        let entry = entry.map_err(|error| io(root, error))?;
        let path = entry.path();
        reject_link(&path)?;
        if entry
            .file_type()
            .map_err(|error| io(&path, error))?
            .is_file()
            && path.extension().and_then(|value| value.to_str()) == Some("md")
        {
            output.push(path);
        }
    }
    output.sort();
    Ok(output)
}

fn declared_skill_files(root: &Path) -> LegacyMigrationResult<(Vec<PathBuf>, Vec<String>)> {
    let mut files = Vec::new();
    let mut skipped = Vec::new();
    for entry in fs::read_dir(root).map_err(|error| io(root, error))? {
        let entry = entry.map_err(|error| io(root, error))?;
        let path = entry.path();
        reject_link(&path)?;
        let name = entry.file_name().to_string_lossy().to_string();
        let kind = entry.file_type().map_err(|error| io(&path, error))?;
        if kind.is_file() && name == "SKILL.md" {
            files.push(path);
        } else if kind.is_dir() && SKILL_ALLOWED_DIRECTORIES.contains(&name.as_str()) {
            collect_regular_files(root, &path, &mut files)?;
        } else {
            skipped.push(name);
        }
    }
    enforce_tree_limits(root, &files)?;
    if !files.iter().any(|path| path == &root.join("SKILL.md")) {
        return Err(LegacyMigrationError::InvalidRequest(format!(
            "Skill {} does not contain SKILL.md",
            root.display()
        )));
    }
    Ok((files, skipped))
}

fn declared_flat_files(
    root: &Path,
    allowed: &[&str],
) -> LegacyMigrationResult<(Vec<PathBuf>, Vec<String>)> {
    let mut files = Vec::new();
    let mut skipped = Vec::new();
    for entry in fs::read_dir(root).map_err(|error| io(root, error))? {
        let entry = entry.map_err(|error| io(root, error))?;
        let path = entry.path();
        reject_link(&path)?;
        let name = entry.file_name().to_string_lossy().to_string();
        if entry
            .file_type()
            .map_err(|error| io(&path, error))?
            .is_file()
            && allowed.contains(&name.as_str())
        {
            files.push(path);
        } else {
            skipped.push(name);
        }
    }
    enforce_tree_limits(root, &files)?;
    Ok((files, skipped))
}

fn collect_regular_files(
    declared_root: &Path,
    current: &Path,
    output: &mut Vec<PathBuf>,
) -> LegacyMigrationResult<()> {
    reject_link(current)?;
    for entry in fs::read_dir(current).map_err(|error| io(current, error))? {
        let entry = entry.map_err(|error| io(current, error))?;
        let path = entry.path();
        reject_link(&path)?;
        let kind = entry.file_type().map_err(|error| io(&path, error))?;
        if kind.is_dir() {
            collect_regular_files(declared_root, &path, output)?;
        } else if kind.is_file() {
            output.push(path);
        }
        if output.len() > MAX_FILES_PER_EXTENSION {
            return Err(LegacyMigrationError::ResourceLimit(format!(
                "extension exceeds {MAX_FILES_PER_EXTENSION} files: {}",
                declared_root.display()
            )));
        }
    }
    Ok(())
}

fn enforce_tree_limits(root: &Path, files: &[PathBuf]) -> LegacyMigrationResult<()> {
    if files.len() > MAX_FILES_PER_EXTENSION {
        return Err(LegacyMigrationError::ResourceLimit(format!(
            "extension exceeds {MAX_FILES_PER_EXTENSION} files: {}",
            root.display()
        )));
    }
    let mut total = 0u64;
    for path in files {
        let bytes = fs::metadata(path).map_err(|error| io(path, error))?.len();
        if bytes > MAX_FILE_BYTES {
            return Err(LegacyMigrationError::ResourceLimit(format!(
                "extension file exceeds {MAX_FILE_BYTES} bytes: {}",
                path.display()
            )));
        }
        total = total.saturating_add(bytes);
    }
    if total > MAX_EXTENSION_BYTES {
        return Err(LegacyMigrationError::ResourceLimit(format!(
            "extension exceeds {MAX_EXTENSION_BYTES} bytes: {}",
            root.display()
        )));
    }
    Ok(())
}

fn copy_declared_tree(
    source_root: &Path,
    target_root: &Path,
    files: &[PathBuf],
) -> LegacyMigrationResult<()> {
    for source in files {
        let relative = source
            .strip_prefix(source_root)
            .map_err(|_| LegacyMigrationError::PathEscape(source.to_path_buf()))?;
        let target = target_root.join(relative);
        let bytes = fs::read(source).map_err(|error| io(source, error))?;
        atomic_write_bytes(&target, &bytes)?;
    }
    Ok(())
}

fn copy_directory(source: &Path, target: &Path) -> LegacyMigrationResult<()> {
    let mut files = Vec::new();
    collect_regular_files(source, source, &mut files)?;
    enforce_tree_limits(source, &files)?;
    copy_declared_tree(source, target, &files)
}

fn resolve_directory_conflict(
    source_id: &str,
    source_hash: &str,
    target: &Path,
) -> LegacyMigrationResult<(String, ImportAction)> {
    if !target.exists() {
        return Ok((source_id.to_string(), ImportAction::Import));
    }
    reject_link(target)?;
    let target_hash = hash_tree(target)?;
    if target_hash == source_hash {
        Ok((source_id.to_string(), ImportAction::Duplicate))
    } else {
        Ok((remapped_id(source_id, source_hash), ImportAction::Remap))
    }
}

fn resolve_miniapp_conflict(
    source: &Path,
    source_id: &str,
    source_hash: &str,
    target_root: &Path,
) -> LegacyMigrationResult<(String, ImportAction)> {
    let target = target_root.join(source_id);
    if !target.exists() {
        return Ok((source_id.to_string(), ImportAction::Import));
    }
    reject_link(&target)?;
    if current_miniapp_matches_legacy(source, &target, source_id)? {
        return Ok((source_id.to_string(), ImportAction::Duplicate));
    }
    let remapped = remapped_id(source_id, source_hash);
    let remapped_target = target_root.join(&remapped);
    if remapped_target.exists() {
        reject_link(&remapped_target)?;
        if current_miniapp_matches_legacy(source, &remapped_target, &remapped)? {
            return Ok((remapped, ImportAction::Duplicate));
        }
        return Err(LegacyMigrationError::InvalidRequest(format!(
            "remapped MiniApp target already contains different data: {}",
            remapped_target.display()
        )));
    }
    Ok((remapped, ImportAction::Remap))
}

fn resolve_remapped_miniapp_conflict(
    source: &Path,
    source_id: &str,
    source_hash: &str,
    target_root: &Path,
) -> LegacyMigrationResult<(String, ImportAction)> {
    let remapped = remapped_id(source_id, source_hash);
    let target = target_root.join(&remapped);
    if !target.exists() {
        return Ok((remapped, ImportAction::Remap));
    }
    reject_link(&target)?;
    if current_miniapp_matches_legacy(source, &target, &remapped)? {
        Ok((remapped, ImportAction::Duplicate))
    } else {
        Err(LegacyMigrationError::InvalidRequest(format!(
            "remapped MiniApp target already contains different data: {}",
            target.display()
        )))
    }
}

fn current_miniapp_matches_legacy(
    source: &Path,
    target: &Path,
    target_id: &str,
) -> LegacyMigrationResult<bool> {
    let source_meta = fs::read_to_string(source.join(META_JSON))
        .map_err(|error| io(&source.join(META_JSON), error))?;
    let plan = build_import_bundle_plan(target_id, &source_meta, 0).map_err(|error| {
        LegacyMigrationError::InvalidRequest(format!(
            "legacy MiniApp {target_id} failed current owner conversion: {error}"
        ))
    })?;
    let expected_meta: serde_json::Value =
        serde_json::from_str(&plan.meta_json).map_err(|error| {
            LegacyMigrationError::InvalidRequest(format!("invalid converted MiniApp meta: {error}"))
        })?;
    let actual_meta: serde_json::Value = match read_bounded_json(target, &target.join(META_JSON)) {
        Ok(value) => value,
        Err(_) => return Ok(false),
    };
    if actual_meta != expected_meta {
        return Ok(false);
    }
    for name in REQUIRED_SOURCE_FILES {
        let source_path = source.join(name);
        let expected = if source_path.exists() {
            fs::read(&source_path).map_err(|error| io(&source_path, error))?
        } else {
            Vec::new()
        };
        let target_path = target.join(SOURCE_DIR).join(name);
        if fs::read(&target_path).ok().as_deref() != Some(expected.as_slice()) {
            return Ok(false);
        }
    }
    let expected_esm = optional_bytes_or(
        &source.join(ESM_DEPS_JSON),
        plan.esm_dependencies_json.as_bytes(),
    )?;
    if fs::read(target.join(SOURCE_DIR).join(ESM_DEPS_JSON))
        .ok()
        .as_deref()
        != Some(expected_esm.as_slice())
    {
        return Ok(false);
    }
    for (name, fallback) in [
        (PACKAGE_JSON, plan.package_json.as_bytes()),
        (STORAGE_JSON, plan.storage_json.as_bytes()),
    ] {
        let expected = optional_bytes_or(&source.join(name), fallback)?;
        if fs::read(target.join(name)).ok().as_deref() != Some(expected.as_slice()) {
            return Ok(false);
        }
    }
    Ok(fs::read(target.join(COMPILED_HTML)).ok().as_deref() == Some(plan.compiled_html.as_bytes()))
}

fn optional_bytes_or(path: &Path, fallback: &[u8]) -> LegacyMigrationResult<Vec<u8>> {
    match fs::read(path) {
        Ok(bytes) => Ok(bytes),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(fallback.to_vec()),
        Err(error) => Err(io(path, error)),
    }
}

fn install_directory_idempotent(
    staged: &Path,
    target: &Path,
    expected_hash: &str,
    run_id: &str,
) -> LegacyMigrationResult<()> {
    if target.exists() {
        if hash_tree(target)? == expected_hash {
            return Ok(());
        }
        return Err(LegacyMigrationError::InvalidRequest(format!(
            "target changed after planning: {}",
            target.display()
        )));
    }
    let parent = target.parent().ok_or_else(|| {
        LegacyMigrationError::InvalidRequest(format!("target has no parent: {}", target.display()))
    })?;
    fs::create_dir_all(parent).map_err(|error| io(parent, error))?;
    let temp = parent.join(format!(
        ".migration-{}-{}",
        safe_component(run_id),
        target.file_name().unwrap_or_default().to_string_lossy()
    ));
    if temp.exists() {
        fs::remove_dir_all(&temp).map_err(|error| io(&temp, error))?;
    }
    copy_directory(staged, &temp)?;
    fs::rename(&temp, target).map_err(|error| io(target, error))
}

fn install_file_idempotent(
    staged: &Path,
    target: &Path,
    expected_hash: &str,
) -> LegacyMigrationResult<()> {
    if target.exists() {
        if hash_file(target)? == expected_hash {
            return Ok(());
        }
        return Err(LegacyMigrationError::InvalidRequest(format!(
            "target changed after planning: {}",
            target.display()
        )));
    }
    let bytes = fs::read(staged).map_err(|error| io(staged, error))?;
    atomic_write_bytes(target, &bytes)
}

fn rollback_imported_directories(
    context: &DomainContext<'_>,
    name: &str,
    target_root: &Path,
) -> LegacyMigrationResult<()> {
    let manifest_path = stage_domain_dir(context, name).join("manifest.json");
    if !manifest_path.exists() {
        return Ok(());
    }
    let manifest = read_manifest(context, name)?;
    for entry in imported_entries(&manifest) {
        let target = target_root.join(&entry.target_id);
        if target.exists() {
            let expected = if name == "miniapps" {
                let staged = stage_domain_dir(context, name)
                    .join("output")
                    .join(&entry.target_id);
                hash_declared_current_miniapp(&staged)?
            } else {
                entry.content_hash.clone()
            };
            if hash_tree(&target)? == expected {
                fs::remove_dir_all(&target).map_err(|error| io(&target, error))?;
            }
        }
    }
    Ok(())
}

fn rollback_builtin_storage(context: &DomainContext<'_>) -> LegacyMigrationResult<()> {
    let manifest_path = stage_domain_dir(context, "miniapps").join("manifest.json");
    if !manifest_path.exists() {
        return Ok(());
    }
    let manifest = read_manifest(context, "miniapps")?;
    let target_root = target_miniapps_root(context.roots);
    for entry in manifest
        .entries
        .iter()
        .filter(|entry| entry.action == ImportAction::BuiltinStorage)
    {
        let staged = stage_domain_dir(context, "miniapps")
            .join("builtin-storage")
            .join(&entry.target_id)
            .join(STORAGE_JSON);
        let target = target_root.join(&entry.target_id).join(STORAGE_JSON);
        remove_file_if_matches(&staged, &target)?;
    }
    Ok(())
}

fn remove_file_if_matches(staged: &Path, target: &Path) -> LegacyMigrationResult<()> {
    if staged.exists() && target.exists() && hash_file(staged)? == hash_file(target)? {
        fs::remove_file(target).map_err(|error| io(target, error))?;
    }
    Ok(())
}

fn require_hash(root: &Path, expected: &str) -> LegacyMigrationResult<()> {
    let actual = hash_tree(root)?;
    if actual != expected {
        return Err(LegacyMigrationError::InvalidRequest(format!(
            "staged extension hash mismatch at {}",
            root.display()
        )));
    }
    Ok(())
}

fn hash_declared_current_miniapp(root: &Path) -> LegacyMigrationResult<String> {
    let layout = MiniAppStorageLayout::new(root.parent().unwrap_or(root), file_name(root)?);
    let mut files = vec![
        layout.meta_path(),
        layout.package_json_path(),
        layout.storage_path(),
        layout.compiled_path(),
    ];
    files.extend(
        REQUIRED_SOURCE_FILES
            .iter()
            .map(|name| layout.source_file_path(name)),
    );
    files.push(layout.source_file_path(ESM_DEPS_JSON));
    let files = files
        .into_iter()
        .filter(|path| path.exists())
        .collect::<Vec<_>>();
    hash_file_set(root, &files)
}

fn hash_tree(root: &Path) -> LegacyMigrationResult<String> {
    let mut files = Vec::new();
    collect_regular_files(root, root, &mut files)?;
    enforce_tree_limits(root, &files)?;
    hash_file_set(root, &files)
}

fn hash_file_set(root: &Path, files: &[PathBuf]) -> LegacyMigrationResult<String> {
    let mut relative = files
        .iter()
        .map(|path| {
            path.strip_prefix(root)
                .map(|relative| (relative.to_path_buf(), path.clone()))
                .map_err(|_| LegacyMigrationError::PathEscape(path.clone()))
        })
        .collect::<LegacyMigrationResult<Vec<_>>>()?;
    relative.sort_by(|left, right| left.0.cmp(&right.0));
    let mut hasher = Sha256::new();
    for (relative, path) in relative {
        hasher.update(relative.to_string_lossy().replace('\\', "/").as_bytes());
        hasher.update([0]);
        hasher.update(fs::read(&path).map_err(|error| io(&path, error))?);
        hasher.update([0]);
    }
    Ok(format!("sha256:{}", hex::encode(hasher.finalize())))
}

fn hash_file(path: &Path) -> LegacyMigrationResult<String> {
    fs::read(path)
        .map(|bytes| hash_bytes(&bytes))
        .map_err(|error| io(path, error))
}

fn hash_bytes(bytes: &[u8]) -> String {
    format!("sha256:{}", hex::encode(Sha256::digest(bytes)))
}

fn remapped_id(source_id: &str, content_hash: &str) -> String {
    let suffix = content_hash
        .strip_prefix("sha256:")
        .unwrap_or(content_hash)
        .chars()
        .take(8)
        .collect::<String>();
    let base = safe_component(source_id);
    let base = if base.is_empty() {
        "legacy-item".to_string()
    } else {
        base
    };
    format!("{base}-from-legacy-{suffix}")
}

fn is_safe_component(value: &str) -> bool {
    if value.is_empty() || safe_component(value) != value {
        return false;
    }
    let stem = value
        .split('.')
        .next()
        .unwrap_or_default()
        .to_ascii_uppercase();
    !matches!(
        stem.as_str(),
        "CON"
            | "PRN"
            | "AUX"
            | "NUL"
            | "COM1"
            | "COM2"
            | "COM3"
            | "COM4"
            | "COM5"
            | "COM6"
            | "COM7"
            | "COM8"
            | "COM9"
            | "LPT1"
            | "LPT2"
            | "LPT3"
            | "LPT4"
            | "LPT5"
            | "LPT6"
            | "LPT7"
            | "LPT8"
            | "LPT9"
    )
}

fn safe_component(value: &str) -> String {
    let normalized = value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.') {
                character
            } else {
                '-'
            }
        })
        .collect::<String>();
    normalized.trim_matches(['.', '-']).to_string()
}

fn file_name(path: &Path) -> LegacyMigrationResult<String> {
    path.file_name()
        .and_then(|value| value.to_str())
        .map(str::to_string)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            LegacyMigrationError::InvalidRequest(format!(
                "path has no valid UTF-8 file name: {}",
                path.display()
            ))
        })
}

fn reject_link(path: &Path) -> LegacyMigrationResult<()> {
    let metadata = fs::symlink_metadata(path).map_err(|error| io(path, error))?;
    if metadata.file_type().is_symlink() || is_reparse_point(&metadata) {
        return Err(LegacyMigrationError::LinkedPath(path.to_path_buf()));
    }
    Ok(())
}

#[cfg(windows)]
fn is_reparse_point(metadata: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    metadata.file_attributes() & 0x0400 != 0
}

#[cfg(not(windows))]
fn is_reparse_point(_metadata: &fs::Metadata) -> bool {
    false
}

fn io(path: &Path, error: std::io::Error) -> LegacyMigrationError {
    LegacyMigrationError::InvalidRequest(format!("I/O failed at {}: {error}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::legacy_migration::adapters_for_groups;
    use openbitfun_legacy_migration::{
        probe_legacy_source, CancellationToken, MigrationEngine, NoCrashInjection, ProbeLimits,
    };
    use openbitfun_product_domains::legacy_migration::{MigrationGroupId, MigrationSelection};
    use std::collections::BTreeSet;

    #[test]
    fn extension_group_uses_owner_formats_and_excludes_builtins() {
        let temp = test_tempdir("extensions");
        let roots = fixture_roots(temp.path());
        copy_fixture(&roots);
        let unknown = roots
            .legacy_skills_root
            .join("user-skill")
            .join("undeclared.txt");
        atomic_write_bytes(&unknown, b"private fixture content that must stay excluded").unwrap();
        let source_hashes = [
            hash_tree(&roots.legacy_user_root).unwrap(),
            hash_tree(&roots.legacy_home_root).unwrap(),
            hash_tree(&roots.legacy_ssh_root).unwrap(),
        ];
        let source = probe_legacy_source(&roots, ProbeLimits::default())
            .unwrap()
            .unwrap();
        let selection = MigrationSelection {
            groups: BTreeSet::from([MigrationGroupId::AgentsSkillsAndMiniapps]),
        };
        let engine = MigrationEngine::new(roots.clone(), adapters_for_groups(&selection)).unwrap();
        let plan = engine
            .plan(&source, selection.clone(), &CancellationToken::default())
            .unwrap();
        let report = engine
            .execute(&plan, &CancellationToken::default(), &NoCrashInjection)
            .unwrap();

        assert!(roots
            .target_skills_root
            .join("user-skill/SKILL.md")
            .exists());
        assert!(!roots
            .target_skills_root
            .join(OPENBITFUN_SYSTEM_SKILL_DIR)
            .exists());
        assert!(!roots
            .target_skills_root
            .join("user-skill/undeclared.txt")
            .exists());
        assert!(report
            .domain_results
            .iter()
            .flat_map(|result| &result.warnings)
            .any(|warning| warning.code == "extension_path_not_declared"));
        assert!(!serde_json::to_string(&report)
            .unwrap()
            .contains("private fixture content"));
        let custom = target_miniapps_root(&roots).join("custom-notes");
        assert!(custom.join("source/index.html").exists());
        assert!(custom.join(COMPILED_HTML).exists());
        assert!(!target_miniapps_root(&roots)
            .join("builtin-gomoku/index.html")
            .exists());
        assert!(target_miniapps_root(&roots)
            .join("builtin-gomoku/storage.json")
            .exists());
        let agent_path = roots.target_user_root.join("agents/researcher.md");
        let agent = custom_agent_read_markdown_str(
            &fs::read_to_string(agent_path).unwrap(),
            CustomAgentLevel::User,
        )
        .unwrap();
        assert_eq!(agent.definition.id, "researcher");

        let second_source = probe_legacy_source(&roots, ProbeLimits::default())
            .unwrap()
            .unwrap();
        let second = engine
            .plan(&second_source, selection, &CancellationToken::default())
            .unwrap();
        assert!(second
            .conflicts
            .iter()
            .any(|conflict| conflict.resolution == ConflictResolution::TargetWins));
        engine
            .execute(&second, &CancellationToken::default(), &NoCrashInjection)
            .unwrap();
        assert_eq!(
            direct_child_directories(&roots.target_skills_root)
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            direct_markdown_files(&roots.target_user_root.join("agents"))
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            [
                hash_tree(&roots.legacy_user_root).unwrap(),
                hash_tree(&roots.legacy_home_root).unwrap(),
                hash_tree(&roots.legacy_ssh_root).unwrap(),
            ],
            source_hashes
        );
    }

    #[test]
    fn conflicting_skill_is_remapped_without_overwriting_target() {
        let temp = test_tempdir("skill-conflict");
        let roots = fixture_roots(temp.path());
        copy_fixture(&roots);
        let target = roots.target_skills_root.join("user-skill/SKILL.md");
        atomic_write_bytes(
            &target,
            b"---\nname: user-skill\ndescription: target\n---\n\nTarget body.\n",
        )
        .unwrap();
        let source = probe_legacy_source(&roots, ProbeLimits::default())
            .unwrap()
            .unwrap();
        let selection = MigrationSelection {
            groups: BTreeSet::from([MigrationGroupId::AgentsSkillsAndMiniapps]),
        };
        let engine = MigrationEngine::new(roots.clone(), adapters_for_groups(&selection)).unwrap();
        let plan = engine
            .plan(&source, selection, &CancellationToken::default())
            .unwrap();
        assert!(plan
            .conflicts
            .iter()
            .any(|conflict| conflict.resolution == ConflictResolution::SourceRemapped));
        engine
            .execute(&plan, &CancellationToken::default(), &NoCrashInjection)
            .unwrap();
        assert!(fs::read_to_string(&target).unwrap().contains("Target body"));
        assert_eq!(
            direct_child_directories(&roots.target_skills_root)
                .unwrap()
                .len(),
            2
        );
    }

    #[test]
    fn unsafe_miniapp_id_is_remapped_before_target_path_construction() {
        let temp = test_tempdir("path");
        let roots = fixture_roots(temp.path());
        copy_fixture(&roots);
        let meta = roots
            .legacy_user_root
            .join("data/miniapps/custom-notes/meta.json");
        let mut meta_value: serde_json::Value =
            serde_json::from_slice(&fs::read(&meta).unwrap()).unwrap();
        meta_value["id"] = serde_json::Value::String("../..".to_string());
        atomic_write_json(&meta, &meta_value).unwrap();
        let source = probe_legacy_source(&roots, ProbeLimits::default())
            .unwrap()
            .unwrap();
        let selection = MigrationSelection {
            groups: BTreeSet::from([MigrationGroupId::AgentsSkillsAndMiniapps]),
        };
        let engine = MigrationEngine::new(roots.clone(), adapters_for_groups(&selection)).unwrap();
        let plan = engine
            .plan(&source, selection, &CancellationToken::default())
            .unwrap();
        assert!(plan.conflicts.iter().any(|conflict| {
            conflict.domain == MigrationDomainId::Miniapps
                && conflict.resolution == ConflictResolution::SourceRemapped
        }));
        engine
            .execute(&plan, &CancellationToken::default(), &NoCrashInjection)
            .unwrap();
        assert!(!roots.target_user_root.join(META_JSON).exists());
        assert!(!roots.target_user_root.join(SOURCE_DIR).exists());
        assert!(direct_child_directories(&target_miniapps_root(&roots))
            .unwrap()
            .iter()
            .any(|path| file_name(path)
                .unwrap()
                .starts_with("legacy-item-from-legacy-")));
    }

    #[test]
    fn rollback_removes_only_matching_builtin_storage() {
        let temp = test_tempdir("builtin-storage-rollback");
        let staged = temp.path().join("staged/storage.json");
        let target = temp.path().join("target/storage.json");
        atomic_write_bytes(&staged, br#"{"value":"legacy"}"#).unwrap();
        atomic_write_bytes(&target, br#"{"value":"legacy"}"#).unwrap();
        remove_file_if_matches(&staged, &target).unwrap();
        assert!(!target.exists());

        atomic_write_bytes(&target, br#"{"value":"target"}"#).unwrap();
        remove_file_if_matches(&staged, &target).unwrap();
        assert_eq!(fs::read(&target).unwrap(), br#"{"value":"target"}"#);
    }

    fn fixture_roots(root: &Path) -> MigrationRoots {
        let legacy_user_root = root.join("legacy-user");
        MigrationRoots {
            legacy_skills_root: legacy_user_root.join("skills"),
            legacy_user_root,
            legacy_home_root: root.join("legacy-home"),
            legacy_ssh_root: root.join("legacy-ssh"),
            target_user_root: root.join("target-user"),
            target_home_root: root.join("target-home"),
            target_skills_root: root.join("target-skills"),
            target_ssh_root: root.join("target-ssh"),
        }
    }

    fn copy_fixture(roots: &MigrationRoots) {
        let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../services/legacy-migration/tests/fixtures/v0.2.19");
        copy_directory(&fixture.join("user-root"), &roots.legacy_user_root).unwrap();
        copy_directory(&fixture.join("home"), &roots.legacy_home_root).unwrap();
        copy_directory(&fixture.join("ssh"), &roots.legacy_ssh_root).unwrap();
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
}
