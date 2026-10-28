//! Read-only LNFU subagent projection. Definitions are read as literal text only:
//! no prompt is executed, resolved, or activated, and no LNFU runtime is started.
//! This module is a source provider only; registration stays with the host.
use openbitfun_product_domains::external_sources::*;
use openbitfun_product_domains::external_subagents::*;
use openbitfun_product_domains::tool_permissions::PermissionConstraintLayer;
use openbitfun_static_hook_support::{read_bounded_text, regular_file_exists, BoundedTextRead};
use serde_json::Value;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

const PROVIDER: &str = "lnfu.subagents";
const ECOSYSTEM: &str = "lnfu";
const MAX_BYTES: usize = 1024 * 1024;
const MAX_ENTRIES: usize = 256;

/// Marker path segment whose presence proves a document executes LNFU logic.
const MARKER_SEGMENT: &str = "lnfu";

pub struct LnfuSubagentProviderOptions {
    pub lnfu_home: PathBuf,
}

impl Default for LnfuSubagentProviderOptions {
    fn default() -> Self {
        let home = dirs::home_dir().unwrap_or_default();
        let lnfu_home = std::env::var("LNFU_HOME")
            .ok()
            .filter(|value| !value.is_empty())
            .map(|value| expand_path(&value, &home))
            .unwrap_or_else(|| home.join(".lnfu"));
        Self { lnfu_home }
    }
}

/// Source provider for LNFU subagent definitions.
///
/// The handle owns no shared mutable state; per-discovery state is created inside
/// `discover`, so one provider can be shared across threads.
#[derive(Default)]
pub struct LnfuSubagentProvider {
    options: LnfuSubagentProviderOptions,
    generation: Arc<AtomicU64>,
}

impl LnfuSubagentProvider {
    pub fn new(options: LnfuSubagentProviderOptions) -> Self {
        Self {
            options,
            generation: Arc::new(AtomicU64::new(0)),
        }
    }
}

fn expand_path(value: &str, home: &Path) -> PathBuf {
    if value == "~" {
        home.to_path_buf()
    } else if let Some(path) = value.strip_prefix("~/") {
        home.join(path)
    } else {
        PathBuf::from(value)
    }
}

fn error(suffix: &str, message: &str, transient: bool) -> ExternalSourceProviderError {
    ExternalSourceProviderError::new(format!("lnfu.subagent.{suffix}"), message, transient)
}

fn read(path: &Path) -> Result<Option<String>, ExternalSourceProviderError> {
    match regular_file_exists(path) {
        Ok(true) => {}
        Ok(false) => return Ok(None),
        Err(_) => return Err(error("read_failed", "Could not inspect LNFU subagent sources", true)),
    }
    match read_bounded_text(path, MAX_BYTES) {
        Ok(BoundedTextRead::Content(text)) => Ok(Some(text)),
        Ok(BoundedTextRead::TooLarge) => Err(error(
            "file_limit",
            "LNFU subagent source exceeds the static discovery size limit",
            false,
        )),
        Ok(BoundedTextRead::InvalidUtf8) => Err(error(
            "source_invalid_utf8",
            "LNFU subagent source is not valid UTF-8",
            false,
        )),
        Err(_) => Err(error("read_failed", "Could not read LNFU subagent sources", true)),
    }
}

/// Per-discovery projection state. Kept in one place so partial health stays
/// attributable after a per-file rejection.
struct Projection {
    provider: ExternalSubagentProviderIdentity,
    generation: u64,
    sources: Vec<ExternalSourceRecord>,
    definitions: Vec<ExternalSubagentDefinition>,
    diagnostics: Vec<ExternalSourceDiagnostic>,
}

impl ExternalSubagentSourceProvider for LnfuSubagentProvider {
    fn identity(&self) -> ExternalSubagentProviderIdentity {
        ExternalSubagentProviderIdentity::new(PROVIDER, ECOSYSTEM, "LNFU Subagents")
            .expect("static provider")
    }

    fn discover(
        &self,
        input: &ExternalSubagentDiscoveryInput,
    ) -> Result<ExternalSubagentProviderSnapshot, ExternalSourceProviderError> {
        let context = &input.context;
        if !self.options.lnfu_home.is_absolute()
            || context
                .workspace_root
                .as_ref()
                .is_some_and(|path| !path.is_absolute())
        {
            return Err(error(
                "path_invalid",
                "LNFU home and workspace paths must be absolute",
                false,
            ));
        }
        let mut files = vec![(
            self.options.lnfu_home.join("config/agents.json"),
            self.options.lnfu_home.clone(),
            ExternalSourceScope::UserGlobal,
        )];
        if let Some(root) = &context.workspace_root {
            files.push((
                root.join(".lnfu/agents.json"),
                root.join(".lnfu"),
                ExternalSourceScope::Project,
            ));
        }
        let mut layers = vec![(
            self.options.lnfu_home.join("agents"),
            ExternalSourceScope::UserGlobal,
        )];
        if let Some(root) = &context.workspace_root {
            layers.push((root.join(".lnfu/agents"), ExternalSourceScope::Project));
        }
        for (root, scope) in layers {
            for path in list_directory(&root)? {
                files.push((path, root.clone(), scope));
            }
        }
        let mut projection = Projection {
            provider: self.identity(),
            generation: self.generation.load(Ordering::Relaxed),
            sources: Vec::new(),
            definitions: Vec::new(),
            diagnostics: Vec::new(),
        };
        let mut seen = BTreeSet::new();
        let mut seen_candidates = BTreeSet::new();
        for (path, _root, scope) in files {
            if !seen.insert(path.clone()) {
                continue;
            }
            let Some(text) = read(&path)? else {
                continue;
            };
            let location = path.to_string_lossy().into_owned();
            let key = SourceKey::new(PROVIDER, location.clone())
                .expect("absolute file paths are valid source identifiers");
            if input.suppressed_sources.contains(&key) {
                continue;
            }
            let index = projection.sources.len();
            projection.sources.push(ExternalSourceRecord {
                key: key.clone(),
                ecosystem_id: EcosystemId::new(ECOSYSTEM).expect("static ecosystem"),
                display_name: display_name(&path),
                source_kind: "subagents_file".to_string(),
                scope,
                location,
                execution_domain_id: context.execution_domain_id.clone(),
                health: ExternalSourceHealth::Available,
                content_version: revision(&text),
                diagnostics: Vec::new(),
            });
            inspect(&text, &key, index, &mut projection, &mut seen_candidates)?;
        }
        Ok(ExternalSubagentProviderSnapshot {
            provider: projection.provider,
            sources: projection.sources,
            definitions: projection.definitions,
            diagnostics: projection.diagnostics,
        })
    }

    fn watch_roots(&self, context: &ExternalSourceContext) -> Vec<ExternalWatchRoot> {
        let mut roots = vec![ExternalWatchRoot {
            path: self.options.lnfu_home.join("agents"),
            recursive: true,
        }];
        if let Some(root) = &context.workspace_root {
            roots.push(ExternalWatchRoot {
                path: root.join(".lnfu/agents"),
                recursive: true,
            });
        }
        roots
    }
}

/// Lists declarative document paths from one directory. A missing directory is an
/// empty layer; every other failure stays actionable instead of silently empty.
fn list_directory(
    directory: &Path,
) -> Result<Vec<PathBuf>, ExternalSourceProviderError> {
    let entries = match std::fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(read_error) if read_error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(_) => {
            return Err(error(
                "read_failed",
                "Could not list LNFU subagent sources",
                true,
            ))
        }
    };
    let mut paths = Vec::new();
    for entry in entries.take(MAX_ENTRIES + 1) {
        paths.push(
            entry
                .map_err(|_| error("read_failed", "Could not list LNFU subagent sources", true))?
                .path(),
        );
    }
    if paths.len() > MAX_ENTRIES {
        return Err(error("source_limit", "LNFU subagent source limit reached", false));
    }
    paths.retain(|path| {
        matches!(
            path.extension().and_then(|value| value.to_str()),
            Some("json" | "toml")
        )
    });
    paths.sort();
    Ok(paths)
}

/// Registers only statically provable facts. Anything whose effect depends on
/// executing LNFU stays visible as invalid instead of being guessed.
fn inspect(
    text: &str,
    key: &SourceKey,
    index: usize,
    projection: &mut Projection,
    seen_candidates: &mut BTreeSet<ExternalSubagentCandidateId>,
) -> Result<(), ExternalSourceProviderError> {
    let document = match serde_json::from_str::<Value>(text) {
        Ok(value) => value,
        Err(_) => {
            reject(
                projection,
                index,
                "lnfu.subagent.declaration_invalid",
                "LNFU subagent declaration could not be parsed as a literal document",
            );
            return Ok(());
        }
    };
    let Some(root) = document.as_object() else {
        reject(
            projection,
            index,
            "lnfu.subagent.declaration_invalid",
            "LNFU subagent declarations must be a literal object",
        );
        return Ok(());
    };
    if let Some(other) = root
        .keys()
        .find(|field| field.as_str() != "agents" && field.as_str() != "version")
    {
        projection.sources[index].health = ExternalSourceHealth::Partial;
        projection.sources[index].diagnostics.push(warning(
            "lnfu.subagent.declaration_scope",
            &format!("LNFU subagent declaration contains unsupported field {other}"),
            key,
        ));
    }
    let Some(agents) = root.get("agents") else {
        return Ok(());
    };
    let Some(rows) = agents.as_array() else {
        reject(
            projection,
            index,
            "lnfu.subagent.declaration_invalid",
            "LNFU subagent declarations must be a literal array",
        );
        return Ok(());
    };
    if rows.len() > MAX_ENTRIES {
        projection.sources[index].health = ExternalSourceHealth::Partial;
        projection.sources[index].diagnostics.push(warning(
            "lnfu.subagent.entry_limit",
            "LNFU subagent declaration limit reached",
            key,
        ));
    }
    for row in rows.iter().take(MAX_ENTRIES) {
        let Some(row) = row.as_object() else {
            projection.sources[index].health = ExternalSourceHealth::Partial;
            projection.sources[index].diagnostics.push(warning(
                "lnfu.subagent.entry_invalid",
                "LNFU subagent entries must be literal objects",
                key,
            ));
            continue;
        };
        let Some(logical_id) = row.get("id").and_then(Value::as_str) else {
            projection.sources[index].health = ExternalSourceHealth::Partial;
            projection.sources[index].diagnostics.push(warning(
                "lnfu.subagent.entry_invalid",
                "LNFU subagent entries must declare a literal identifier",
                key,
            ));
            continue;
        };
        let local_id = match ExternalSubagentLocalId::new(logical_id) {
            Ok(local_id) => local_id,
            Err(_) => {
                projection.sources[index].health = ExternalSourceHealth::Partial;
                projection.sources[index].diagnostics.push(warning(
                    "lnfu.subagent.entry_invalid",
                    "LNFU subagent identifier is not a valid literal identifier",
                    key,
                ));
                continue;
            }
        };
        let contribution_id = ExternalSubagentContributionId::new(key.clone(), local_id);
        let provenance = vec![ExternalSubagentProvenanceRef {
            contribution_id,
            role: ExternalSubagentContributionRole::Definition,
        }];
        let candidate_id = external_subagent_candidate_id(&projection.provider.provider_id, logical_id, &provenance);
        let display_name = literal_text(row.get("name").and_then(Value::as_str));
        let description = literal_text(row.get("description").and_then(Value::as_str));
        let prompt = SecretText::new(literal_text(row.get("prompt").and_then(Value::as_str)));
        let disabled = row.get("disabled").and_then(Value::as_bool) == Some(true);
        let hidden = row.get("hidden").and_then(Value::as_bool) == Some(true);
        // A definition tool selection is enforced at launch; projecting it as an
        // unenforceable set would publish a partially enforced constraint, so it
        // is reported as a degraded definition instead.
        let has_tool_selection =
            row.get("tools").is_some() || row.get("allowedTools").is_some();
        // Every projected prompt is inert: LNFU semantics require its runtime,
        // so LNFU-specific phrasing is reported as a blocked definition.
        let (compatibility, diagnostic_codes) = if executes_lnfu(text) {
            (
                ExternalSubagentCompatibilityState::Blocked,
                vec!["lnfu.subagent.runtime_required".to_string()],
            )
        } else if has_tool_selection {
            (ExternalSubagentCompatibilityState::ReadyWithDegradation, Vec::new())
        } else {
            (ExternalSubagentCompatibilityState::Ready, Vec::new())
        };
        if !seen_candidates.insert(candidate_id.clone()) {
            projection.sources[index].health = ExternalSourceHealth::Partial;
            projection.sources[index].diagnostics.push(warning(
                "lnfu.subagent.entry_duplicate",
                "LNFU subagent declarations repeat one candidate identity",
                key,
            ));
            continue;
        }
        let definition = ExternalSubagentDefinition {
            candidate_id,
            logical_id: logical_id.to_string(),
            provenance,
            display_name,
            description,
            prompt,
            mode: match row.get("mode").and_then(Value::as_str) {
                Some("primary") => ExternalSubagentMode::Primary,
                Some("all") => ExternalSubagentMode::All,
                _ => ExternalSubagentMode::Subagent,
            },
            disabled,
            hidden,
            requested_model: ExternalSubagentModelRequest::default(),
            requested_model_profile: None,
            requested_tools: ExternalSubagentToolRequest {
                selectors: Vec::new(),
                uses_conservative_default: true,
            },
            permission_constraints: PermissionConstraintLayer::default(),
            compatibility,
            diagnostic_codes,
            behavior_version: ExternalSubagentBehaviorVersion::new(format!(
                "lnfu-static-{}",
                projection.generation
            ))
            .expect("static behavior version"),        };
        projection.definitions.push(definition);
    }
    Ok(())
}

fn warning(code: &str, message: &str, source: &SourceKey) -> ExternalSourceDiagnostic {
    ExternalSourceDiagnostic::warning(code, message, Some(source.clone()))
}

fn reject(
    projection: &mut Projection,
    index: usize,
    code: &str,
    message: &str,
) {
    let Some(source) = projection.sources.get(index) else {
        return;
    };
    let source = source.clone();
    projection.sources[index].health = ExternalSourceHealth::Unavailable;
    let diagnostic = ExternalSourceDiagnostic::error(code, message, Some(source.key.clone()));
    projection.sources[index].diagnostics.push(diagnostic.clone());
    projection.diagnostics.push(diagnostic);
}

/// True when a document literally names the LNFU runtime, which alone decides
/// the effective prompt. The check is evidence-driven, never a heuristic.
fn executes_lnfu(text: &str) -> bool {
    text.split(|character: char| !character.is_ascii_alphanumeric())
        .any(|segment| segment.eq_ignore_ascii_case(MARKER_SEGMENT))
}

fn literal_text(value: Option<&str>) -> String {
    value
        .filter(|value| value.len() <= 4096 && !value.chars().any(char::is_control))
        .unwrap_or_default()
        .to_string()
}

fn display_name(path: &Path) -> String {
    path.file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("agents")
        .to_string()
}

/// Content revision derived from literal bytes so a changed declaration always
/// produces a changed version without reading the file twice.
fn revision(text: &str) -> String {
    format!("lnfu-subagents:{}", text.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn context(workspace: Option<PathBuf>) -> ExternalSourceContext {
        ExternalSourceContext {
            workspace_root: workspace,
            execution_domain_id: ExecutionDomainId::new("fixture").unwrap(),
        }
    }

    fn input(workspace: Option<PathBuf>) -> ExternalSubagentDiscoveryInput {
        ExternalSubagentDiscoveryInput {
            context: context(workspace),
            suppressed_sources: BTreeSet::new(),
        }
    }

    fn write(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    fn provider(home: PathBuf) -> LnfuSubagentProvider {
        LnfuSubagentProvider::new(LnfuSubagentProviderOptions { lnfu_home: home })
    }

    #[test]
    fn literal_definitions_are_projected_without_execution() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        write(
            &home.join("config/agents.json"),
            r#"{"version":1,"agents":[
                {"id":"reviewer","name":"Reviewer","description":"Reviews","prompt":"SECRET_PROMPT"}
            ]}"#,
        );
        let snapshot = provider(home).discover(&input(None)).unwrap();
        assert_eq!(snapshot.provider.ecosystem_id.as_str(), "lnfu");
        assert_eq!(snapshot.sources.len(), 1);
        assert_eq!(snapshot.sources[0].health, ExternalSourceHealth::Available);
        assert_eq!(snapshot.definitions.len(), 1);
        assert_eq!(snapshot.definitions[0].logical_id, "reviewer");
        assert_eq!(snapshot.definitions[0].display_name, "Reviewer");
        assert_eq!(snapshot.definitions[0].prompt.expose(), "SECRET_PROMPT");
        assert!(!format!("{:?}", snapshot.definitions[0]).contains("SECRET_PROMPT"));
        let encoded = serde_json::to_string(&snapshot.sources).unwrap();
        assert!(!encoded.contains("SECRET_PROMPT"));
    }

    #[test]
    fn project_directory_declarations_are_discovered_per_file() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        let workspace = temp.path().join("workspace");
        write(
            &workspace.join(".lnfu/agents/lint.json"),
            r#"{"agents":[{"id":"linter","mode":"primary"}]}"#,
        );
        let snapshot = provider(home).discover(&input(Some(workspace))).unwrap();
        assert_eq!(snapshot.sources.len(), 1);
        assert_eq!(snapshot.sources[0].scope, ExternalSourceScope::Project);
        assert_eq!(snapshot.definitions.len(), 1);
        assert_eq!(snapshot.definitions[0].mode, ExternalSubagentMode::Primary);
    }

    #[test]
    fn invalid_declarations_are_rejected_and_partial() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        write(&home.join("config/agents.json"), "not json");
        write(
            &home.join("agents/other.json"),
            r#"{"agents":[{"name":"missing id"}]}"#,
        );
        let snapshot = provider(home).discover(&input(None)).unwrap();
        assert_eq!(snapshot.sources.len(), 2);
        assert!(snapshot
            .sources
            .iter()
            .all(|source| source.health != ExternalSourceHealth::Available));
        assert!(snapshot.definitions.is_empty());
        assert!(snapshot
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "lnfu.subagent.declaration_invalid"));
    }

    #[test]
    fn runtime_dependent_prompts_block_the_definition() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        write(
            &home.join("config/agents.json"),
            r#"{"agents":[{"id":"wrapped","prompt":"run lnfu tool"}]}"#,
        );
        let snapshot = provider(home).discover(&input(None)).unwrap();
        assert_eq!(snapshot.definitions.len(), 1);
        assert_eq!(
            snapshot.definitions[0].compatibility,
            ExternalSubagentCompatibilityState::Blocked
        );
        assert_eq!(
            snapshot.definitions[0].diagnostic_codes,
            vec!["lnfu.subagent.runtime_required".to_string()]
        );
    }

    #[test]
    fn suppressed_sources_are_not_projected() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        let path = home.join("config/agents.json");
        write(&path, r#"{"agents":[{"id":"reviewer"}]}"#);
        let mut input = input(None);
        input
            .suppressed_sources
            .insert(SourceKey::new(PROVIDER, path.to_string_lossy().into_owned()).unwrap());
        let snapshot = provider(home).discover(&input).unwrap();
        assert!(snapshot.sources.is_empty());
        assert!(snapshot.definitions.is_empty());
    }

    #[test]
    fn watch_roots_cover_user_and_project_layers() {
        let provider = provider(PathBuf::from("C:/lnfu-home"));
        let roots = provider.watch_roots(&context(Some(PathBuf::from("C:/workspace"))));
        assert_eq!(roots.len(), 2);
        assert!(roots.iter().all(|root| root.recursive));
        assert_eq!(roots[0].path, PathBuf::from("C:/lnfu-home/agents"));
        assert_eq!(roots[1].path, PathBuf::from("C:/workspace/.lnfu/agents"));
    }
}
