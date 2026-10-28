//! Static LNFU hook projection. Declarations are read as literal text only:
//! no script is imported, evaluated, launched, or otherwise resolved.
use openbitfun_product_domains::external_hook_catalog::*;
use openbitfun_product_domains::external_sources::*;
use openbitfun_static_hook_support::{
    read_bounded_text, regular_file_exists, BoundedTextRead, StaticHookCatalog,
};
use serde_json::Value;
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};

const PROVIDER: &str = "lnfu.hooks";
const ECOSYSTEM: &str = "lnfu";
const MAX_BYTES: usize = 1024 * 1024;
const MAX_ENTRIES: usize = 256;

#[derive(Debug, Clone)]
pub struct LnfuHookProviderOptions {
    pub lnfu_home: PathBuf,
}

impl Default for LnfuHookProviderOptions {
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

#[derive(Default)]
pub struct LnfuHookProvider {
    options: LnfuHookProviderOptions,
}

impl LnfuHookProvider {
    pub fn new(options: LnfuHookProviderOptions) -> Self {
        Self { options }
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
    ExternalSourceProviderError::new(format!("lnfu.hook.{suffix}"), message, transient)
}

fn read(path: &Path) -> Result<Option<String>, ExternalSourceProviderError> {
    match regular_file_exists(path) {
        Ok(true) => {}
        Ok(false) => return Ok(None),
        Err(_) => {
            return Err(error(
                "read_failed",
                "Could not inspect LNFU Hook sources",
                true,
            ))
        }
    }
    match read_bounded_text(path, MAX_BYTES) {
        Ok(BoundedTextRead::Content(text)) => Ok(Some(text)),
        Ok(BoundedTextRead::TooLarge) => Err(error(
            "file_limit",
            "LNFU Hook source exceeds the static discovery size limit",
            false,
        )),
        Ok(BoundedTextRead::InvalidUtf8) => Err(error(
            "source_invalid_utf8",
            "LNFU Hook source is not valid UTF-8",
            false,
        )),
        Err(_) => Err(error(
            "read_failed",
            "Could not read LNFU Hook sources",
            true,
        )),
    }
}

impl ExternalHookSourceProvider for LnfuHookProvider {
    fn identity(&self) -> ExternalHookProviderIdentity {
        ExternalHookProviderIdentity::new(PROVIDER, ECOSYSTEM, "LNFU Hooks")
            .expect("static provider")
    }

    fn discover(
        &self,
        context: &ExternalSourceContext,
    ) -> Result<ExternalHookProviderSnapshot, ExternalSourceProviderError> {
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
        let mut catalog = StaticHookCatalog::new(self.identity());
        let mut layers = vec![(
            self.options.lnfu_home.clone(),
            ExternalSourceScope::UserGlobal,
        )];
        if let Some(root) = &context.workspace_root {
            layers.push((root.join(".lnfu"), ExternalSourceScope::Project));
        }
        let mut seen = BTreeSet::new();
        for (root, scope) in layers {
            for name in ["hooks.json", "hooks.toml"] {
                let path = root.join(name);
                let Some(text) = read(&path)? else {
                    continue;
                };
                if !seen.insert(path.clone()) {
                    continue;
                }
                let index = catalog.source(
                    &path.to_string_lossy(),
                    &path.to_string_lossy(),
                    ExternalHookSourceKind::HooksFile,
                    scope,
                )?;
                inspect(&text, &path, index, &mut catalog)?;
            }
            // A hooks directory holds one declaration document per file. Each
            // file becomes its own source so partial health stays attributable.
            let directory = root.join("hooks");
            let entries = match std::fs::read_dir(&directory) {
                Ok(entries) => entries,
                Err(read_error) if read_error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(_) => {
                    return Err(error(
                        "read_failed",
                        "Could not list LNFU Hook sources",
                        true,
                    ))
                }
            };
            let mut paths = Vec::new();
            for entry in entries.take(MAX_ENTRIES + 1) {
                paths.push(
                    entry
                        .map_err(|_| {
                            error("read_failed", "Could not list LNFU Hook sources", true)
                        })?
                        .path(),
                );
            }
            if paths.len() > MAX_ENTRIES {
                return Err(error(
                    "source_limit",
                    "LNFU Hook source limit reached",
                    false,
                ));
            }
            paths.sort();
            for path in paths {
                let extension = path.extension().and_then(|value| value.to_str());
                if !matches!(extension, Some("json" | "toml")) {
                    continue;
                }
                let Some(text) = read(&path)? else {
                    continue;
                };
                if !seen.insert(path.clone()) {
                    continue;
                }
                let index = catalog.source(
                    &path.to_string_lossy(),
                    &path.to_string_lossy(),
                    ExternalHookSourceKind::HooksFile,
                    scope,
                )?;
                inspect(&text, &path, index, &mut catalog)?;
            }
        }
        catalog.finish()
    }
}

/// Registers only statically provable facts. Anything whose effect depends on
/// running LNFU is reported as opaque with an unknown activation state.
fn inspect(
    text: &str,
    path: &Path,
    index: usize,
    catalog: &mut StaticHookCatalog,
) -> Result<(), ExternalSourceProviderError> {
    let toml_source = path.extension().and_then(|value| value.to_str()) == Some("toml");
    let parsed = if toml_source {
        toml::from_str::<toml::Value>(text)
            .ok()
            .and_then(|value| serde_json::to_value(value).ok())
    } else {
        serde_json::from_str::<Value>(text).ok()
    };
    let Some(Value::Object(root)) = parsed else {
        catalog.warning(
            index,
            "lnfu.hook.declaration_invalid",
            "LNFU Hook declaration could not be parsed as a literal document",
        );
        catalog.event(
            index,
            "hooks",
            ExternalHookHandlerKind::Function,
            ExternalHookMatcherSummary::Unavailable,
            ExternalHookProjectionStatus::Opaque,
            ExternalHookNativeActivation::Unknown,
        )?;
        return Ok(());
    };
    if root.keys().any(|key| key != "hooks" && key != "version") {
        catalog.warning(
            index,
            "lnfu.hook.declaration_scope",
            "LNFU Hook declaration contains unsupported top-level fields; only literal hook rows are inspected",
        );
    }
    let rows = match root.get("hooks") {
        None => return Ok(()),
        Some(Value::Array(rows)) => rows,
        Some(_) => {
            catalog.warning(
                index,
                "lnfu.hook.declaration_invalid",
                "LNFU Hook declarations must be a literal array",
            );
            return Ok(());
        }
    };
    if rows.len() > MAX_ENTRIES {
        catalog.warning(
            index,
            "lnfu.hook.entry_limit",
            "LNFU Hook declaration limit reached",
        );
    }
    for row in rows.iter().take(MAX_ENTRIES) {
        let Some(row) = row.as_object() else {
            catalog.warning(
                index,
                "lnfu.hook.entry_invalid",
                "LNFU Hook entries must be literal objects",
            );
            continue;
        };
        let Some(event) = row
            .get("event")
            .and_then(Value::as_str)
            .filter(|event| valid_event(event))
        else {
            catalog.warning(
                index,
                "lnfu.hook.event_invalid",
                "LNFU Hook entries must declare a literal ASCII event name",
            );
            continue;
        };
        // A matcher is only trustworthy when it is a literal string. Any other
        // shape means the effective match cannot be determined statically.
        let matcher = match row.get("matcher") {
            None => ExternalHookMatcherSummary::Any,
            Some(Value::String(value))
                if value.len() <= 512 && !value.chars().any(char::is_control) =>
            {
                if value.contains(['*', '?', '$', '`', '{', '}']) {
                    catalog.warning(
                        index,
                        "lnfu.hook.matcher_dynamic",
                        "Dynamic LNFU Hook matchers require the native LNFU runtime",
                    );
                    ExternalHookMatcherSummary::Dynamic
                } else {
                    ExternalHookMatcherSummary::Pattern {
                        display: value.clone(),
                    }
                }
            }
            Some(_) => ExternalHookMatcherSummary::Dynamic,
        };
        let kind = row.get("type").and_then(Value::as_str);
        let (handler_kind, status) = match kind {
            // `command` names an executable that LNFU launches; discovery never
            // reads or starts it, so the handler stays opaque.
            Some("command") => (
                ExternalHookHandlerKind::Command,
                ExternalHookProjectionStatus::Opaque,
            ),
            Some("http") => (
                ExternalHookHandlerKind::Http,
                ExternalHookProjectionStatus::Opaque,
            ),
            Some("mcp") | Some("mcp-tool") => (
                ExternalHookHandlerKind::McpTool,
                ExternalHookProjectionStatus::Opaque,
            ),
            Some("prompt") => (
                ExternalHookHandlerKind::Prompt,
                ExternalHookProjectionStatus::Opaque,
            ),
            Some("agent") => (
                ExternalHookHandlerKind::Agent,
                ExternalHookProjectionStatus::Opaque,
            ),
            Some("function") | Some("script") | None => (
                ExternalHookHandlerKind::Function,
                ExternalHookProjectionStatus::Opaque,
            ),
            Some(_) => {
                catalog.warning(
                    index,
                    "lnfu.hook.handler_invalid",
                    "LNFU Hook handler type is not a known literal kind",
                );
                (
                    ExternalHookHandlerKind::Function,
                    ExternalHookProjectionStatus::Opaque,
                )
            }
        };
        if matches!(kind, Some("function" | "script")) || kind.is_none() {
            catalog.warning(
                index,
                "lnfu.hook.handler_opaque",
                "LNFU function Hook handlers require the native LNFU runtime to resolve, so they remain opaque",
            );
        }
        catalog.event(
            index,
            event,
            handler_kind,
            matcher,
            status,
            ExternalHookNativeActivation::Unknown,
        )?;
    }
    Ok(())
}

fn valid_event(event: &str) -> bool {
    !event.is_empty()
        && event.len() <= 160
        && event
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
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

    fn write(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    #[test]
    fn static_declarations_are_projected_without_execution() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        write(
            &home.join("hooks.json"),
            r#"{"hooks":[
                {"event":"before_run","matcher":"*.py","type":"command","command":"python SECRET.py"},
                {"event":"after_run","type":"function","handler":"SECRET_HANDLER"}
            ]}"#,
        );
        let snapshot = LnfuHookProvider::new(LnfuHookProviderOptions {
            lnfu_home: home.clone(),
        })
        .discover(&context(None))
        .unwrap();
        assert_eq!(snapshot.provider.ecosystem_id.as_str(), "lnfu");
        assert_eq!(snapshot.entries.len(), 2);
        let before = snapshot
            .entries
            .iter()
            .find(|entry| entry.native_event == "before_run")
            .unwrap();
        assert_eq!(before.handler_kind, ExternalHookHandlerKind::Command);
        assert_eq!(
            before.projection_status,
            ExternalHookProjectionStatus::Opaque
        );
        assert_eq!(
            before.native_activation,
            ExternalHookNativeActivation::Unknown
        );
        assert!(before.mapping.is_none());
        assert!(matches!(
            &before.matcher,
            ExternalHookMatcherSummary::Dynamic
        ));
        let encoded = serde_json::to_string(&snapshot).unwrap();
        assert!(!encoded.contains("SECRET"));
    }

    #[test]
    fn project_scope_directory_declarations_are_discovered_per_file() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        let workspace = temp.path().join("workspace");
        write(
            &workspace.join(".lnfu/hooks/lint.toml"),
            "[[hooks]]\nevent = \"on_save\"\ntype = \"http\"\nurl = \"https://example.test\"\n",
        );
        let snapshot = LnfuHookProvider::new(LnfuHookProviderOptions {
            lnfu_home: home.clone(),
        })
        .discover(&context(Some(workspace)))
        .unwrap();
        assert_eq!(snapshot.sources.len(), 1);
        assert_eq!(snapshot.sources[0].scope, ExternalSourceScope::Project);
        assert_eq!(
            snapshot.sources[0].source_kind,
            ExternalHookSourceKind::HooksFile
        );
        assert_eq!(snapshot.entries.len(), 1);
        assert_eq!(snapshot.entries[0].native_event, "on_save");
        assert_eq!(
            snapshot.entries[0].handler_kind,
            ExternalHookHandlerKind::Http
        );
    }

    #[test]
    fn invalid_or_dynamic_declarations_are_opaque_and_partial() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        write(&home.join("hooks.json"), "not json");
        write(
            &home.join("hooks/other.json"),
            r#"{"hooks":[{"event":"on_save","matcher":"${PATTERN}","type":"script"}]}"#,
        );
        let snapshot = LnfuHookProvider::new(LnfuHookProviderOptions {
            lnfu_home: home.clone(),
        })
        .discover(&context(None))
        .unwrap();
        assert_eq!(snapshot.sources.len(), 2);
        assert!(snapshot
            .sources
            .iter()
            .all(|source| source.health == ExternalSourceHealth::Partial));
        assert!(snapshot.entries.iter().all(|entry| entry.projection_status
            == ExternalHookProjectionStatus::Opaque
            && entry.native_activation == ExternalHookNativeActivation::Unknown));
        assert!(snapshot
            .entries
            .iter()
            .any(|entry| entry.native_event == "on_save"
                && matches!(entry.matcher, ExternalHookMatcherSummary::Dynamic)));
    }

    #[test]
    fn missing_home_yields_an_empty_valid_catalog() {
        let temp = tempfile::tempdir().unwrap();
        let snapshot = LnfuHookProvider::new(LnfuHookProviderOptions {
            lnfu_home: temp.path().join("absent"),
        })
        .discover(&context(None))
        .unwrap();
        assert!(snapshot.sources.is_empty());
        assert!(snapshot.entries.is_empty());
    }

    #[test]
    fn import_projection_is_explicitly_unsupported() {
        let provider = LnfuHookProvider::new(LnfuHookProviderOptions {
            lnfu_home: PathBuf::from("C:/lnfu-home"),
        });
        let error = provider
            .prepare_import(
                &context(None),
                &SourceKey::new(PROVIDER, "missing").unwrap(),
                "sha256:0",
            )
            .unwrap_err();
        assert_eq!(error.code, "external_hook.import_unsupported");
    }
}
