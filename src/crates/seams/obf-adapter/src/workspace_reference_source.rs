//! Static LNFU workspace reference projection.
//!
//! Declared reference directories are read as literal declaration text only:
//! no path is created, resolved for existence, or granted filesystem
//! permission by this provider. A declared path may legitimately not exist yet.
use openbitfun_product_domains::external_sources::*;
use openbitfun_product_domains::workspace_references::*;
use openbitfun_static_hook_support::{read_bounded_text, regular_file_exists, BoundedTextRead};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};

const PROVIDER: &str = "lnfu.workspace_references";
const ECOSYSTEM: &str = "lnfu";
const DISPLAY_NAME: &str = "LNFU Workspace References";
const SOURCE_KIND: &str = "workspace_references";
const MAX_BYTES: usize = 1024 * 1024;
const MAX_FILES: usize = 64;
const MAX_REFERENCES: usize = 256;
const DECLARATION_DOCUMENT: &str = "references.json";

#[derive(Debug, Clone)]
pub struct LnfuWorkspaceReferenceProviderOptions {
    pub lnfu_home: PathBuf,
}

impl Default for LnfuWorkspaceReferenceProviderOptions {
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
pub struct LnfuWorkspaceReferenceProvider {
    options: LnfuWorkspaceReferenceProviderOptions,
}

impl LnfuWorkspaceReferenceProvider {
    pub fn new(options: LnfuWorkspaceReferenceProviderOptions) -> Self {
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
    ExternalSourceProviderError::new(
        format!("lnfu.workspace_reference.{suffix}"),
        message,
        transient,
    )
}

/// Reads one declaration document as bounded literal text. Non-UTF-8 and
/// oversized documents are reported instead of being partially decoded.
fn read(path: &Path) -> Result<Option<String>, ExternalSourceProviderError> {
    match regular_file_exists(path) {
        Ok(true) => {}
        Ok(false) => return Ok(None),
        Err(_) => {
            return Err(error(
                "read_failed",
                "Could not inspect LNFU workspace reference sources",
                true,
            ))
        }
    }
    match read_bounded_text(path, MAX_BYTES) {
        Ok(BoundedTextRead::Content(text)) => Ok(Some(text)),
        Ok(BoundedTextRead::TooLarge) => Err(error(
            "file_limit",
            "LNFU workspace reference source exceeds the static discovery size limit",
            false,
        )),
        Ok(BoundedTextRead::InvalidUtf8) => Err(error(
            "source_invalid_utf8",
            "LNFU workspace reference source is not valid UTF-8",
            false,
        )),
        Err(_) => Err(error(
            "read_failed",
            "Could not read LNFU workspace reference sources",
            true,
        )),
    }
}

struct Declaration {
    identity: String,
    row: Value,
}

/// Extracts literal reference rows from one LNFU workspace reference document.
///
/// The document is `{"references": [ ... ]}`. A non-array container or a
/// non-object top-level shape is reported as a diagnostic and yields no rows,
/// so a malformed document never silently becomes an empty valid catalog.
fn collect_rows(
    text: &str,
    source: &SourceKey,
    catalog: &mut Vec<ExternalSourceDiagnostic>,
) -> Vec<Declaration> {
    let parsed = serde_json::from_str::<Value>(text).ok();
    let Some(Value::Object(root)) = parsed else {
        catalog.push(
            ExternalSourceDiagnostic::warning(
                "lnfu.workspace_reference.declaration_invalid",
                "LNFU workspace reference declaration could not be parsed as a literal document",
                Some(source.clone()),
            )
            .with_asset_kind(ExternalSourceAssetKind::Reference),
        );
        return Vec::new();
    };
    if root
        .keys()
        .any(|key| !matches!(key.as_str(), "references" | "version"))
    {
        catalog.push(
            ExternalSourceDiagnostic::warning(
                "lnfu.workspace_reference.declaration_scope",
                "LNFU workspace reference declaration contains unsupported top-level fields; only literal reference rows are inspected",
                Some(source.clone()),
            )
            .with_asset_kind(ExternalSourceAssetKind::Reference),
        );
    }
    let rows = match root.get("references") {
        None => return Vec::new(),
        Some(Value::Array(rows)) => rows,
        Some(_) => {
            catalog.push(
                ExternalSourceDiagnostic::warning(
                    "lnfu.workspace_reference.declaration_invalid",
                    "LNFU workspace reference declarations must be a literal array",
                    Some(source.clone()),
                )
                .with_asset_kind(ExternalSourceAssetKind::Reference),
            );
            return Vec::new();
        }
    };
    if rows.len() > MAX_REFERENCES {
        catalog.push(
            ExternalSourceDiagnostic::warning(
                "lnfu.workspace_reference.entry_limit",
                "LNFU workspace reference declaration limit reached",
                Some(source.clone()),
            )
            .with_asset_kind(ExternalSourceAssetKind::Reference),
        );
    }
    rows.iter()
        .take(MAX_REFERENCES)
        .enumerate()
        .map(|(index, row)| Declaration {
            identity: format!("/references/{index}"),
            row: row.clone(),
        })
        .collect()
}

fn literal_string(value: Option<&Value>) -> Option<String> {
    match value {
        Some(Value::String(text)) => Some(text.clone()),
        _ => None,
    }
}

/// Renders a bounded, attributable row diagnostic message. The row identity is
/// the document-relative literal path, never row content.
fn row_message(message: &str, identity: &str) -> String {
    format!("{message} (at {identity})")
}

impl ExternalWorkspaceReferenceSourceProvider for LnfuWorkspaceReferenceProvider {
    fn identity(&self) -> ExternalWorkspaceReferenceProviderIdentity {
        ExternalWorkspaceReferenceProviderIdentity::new(PROVIDER, ECOSYSTEM, DISPLAY_NAME)
            .expect("static id")
    }

    fn discover(
        &self,
        context: &ExternalSourceContext,
    ) -> Result<ExternalWorkspaceReferenceProviderSnapshot, ExternalSourceProviderError> {
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
        let provider = self.identity();
        let mut layers = vec![(
            self.options.lnfu_home.clone(),
            ExternalSourceScope::UserGlobal,
        )];
        if let Some(root) = &context.workspace_root {
            layers.push((root.join(".lnfu"), ExternalSourceScope::Project));
        }
        if layers.len() > MAX_FILES {
            return Err(error(
                "source_limit",
                "LNFU workspace reference source limit reached",
                false,
            ));
        }

        let mut sources = Vec::new();
        let mut references = Vec::new();
        let mut diagnostics = Vec::new();
        let mut seen = BTreeSet::new();
        let mut aliases = BTreeSet::new();
        for (root, scope) in layers {
            let path = root.join(DECLARATION_DOCUMENT);
            let Some(text) = read(&path)? else {
                continue;
            };
            if !seen.insert(path.clone()) {
                continue;
            }
            let location = path.to_string_lossy().into_owned();
            let source = SourceKey::new(provider.provider_id.as_str(), location.as_str())
                .expect("static provider id");
            let content_version =
                format!("sha256:{}", hex::encode(Sha256::digest(text.as_bytes())));
            let mut source_diagnostics = Vec::new();
            let declarations = collect_rows(&text, &source, &mut source_diagnostics);
            let health = if source_diagnostics.is_empty() {
                ExternalSourceHealth::Available
            } else {
                ExternalSourceHealth::Partial
            };
            let record = ExternalSourceRecord {
                key: source.clone(),
                ecosystem_id: provider.ecosystem_id.clone(),
                display_name: format!("LNFU workspace references ({})", path.to_string_lossy()),
                source_kind: SOURCE_KIND.to_string(),
                scope,
                location,
                execution_domain_id: context.execution_domain_id.clone(),
                health,
                content_version: content_version.clone(),
                diagnostics: Vec::new(),
            };
            diagnostics.extend(source_diagnostics);
            sources.push(record);

            for declaration in declarations {
                let Some(row) = declaration.row.as_object() else {
                    diagnostics.push(
                        ExternalSourceDiagnostic::warning(
                            "lnfu.workspace_reference.entry_invalid",
                            row_message(
                                "LNFU workspace reference entries must be literal objects",
                                &declaration.identity,
                            ),
                            Some(source.clone()),
                        )
                        .with_asset_kind(ExternalSourceAssetKind::Reference),
                    );
                    continue;
                };
                // Only literal strings are trustworthy. A computed alias or path
                // cannot be resolved statically, so the row stays a diagnostic.
                let Some(alias) = literal_string(row.get("alias")) else {
                    diagnostics.push(
                        ExternalSourceDiagnostic::warning(
                            "lnfu.workspace_reference.alias_invalid",
                            row_message(
                                "LNFU workspace reference entries must declare a literal alias",
                                &declaration.identity,
                            ),
                            Some(source.clone()),
                        )
                        .with_asset_kind(ExternalSourceAssetKind::Reference),
                    );
                    continue;
                };
                let Some(path_text) = literal_string(row.get("path")) else {
                    diagnostics.push(
                        ExternalSourceDiagnostic::warning(
                            "lnfu.workspace_reference.path_invalid",
                            row_message(
                                "LNFU workspace reference entries must declare a literal path",
                                &declaration.identity,
                            ),
                            Some(source.clone()),
                        )
                        .with_asset_kind(ExternalSourceAssetKind::Reference),
                    );
                    continue;
                };
                // Declared paths are taken as declared. Expansion is host-owned:
                // this provider never resolves `~`, environment variables, or
                // existence, because a reference never grants filesystem access.
                let declared = PathBuf::from(path_text);
                if !declared.is_absolute() {
                    diagnostics.push(
                        ExternalSourceDiagnostic::warning(
                            "lnfu.workspace_reference.path_not_absolute",
                            row_message(
                                "LNFU workspace reference path is not absolute",
                                &declaration.identity,
                            ),
                            Some(source.clone()),
                        )
                        .with_asset_kind(ExternalSourceAssetKind::Reference),
                    );
                    continue;
                }
                let description = match row.get("description") {
                    None => None,
                    Some(Value::String(value)) if !value.is_empty() => Some(value.clone()),
                    Some(_) => {
                        diagnostics.push(
                            ExternalSourceDiagnostic::warning(
                                "lnfu.workspace_reference.description_invalid",
                                row_message(
                                    "LNFU workspace reference description must be a literal non-empty string",
                                    &declaration.identity,
                                ),
                                Some(source.clone()),
                            )
                            .with_asset_kind(ExternalSourceAssetKind::Reference),
                        );
                        continue;
                    }
                };
                let hidden = match row.get("hidden") {
                    None => false,
                    Some(Value::Bool(value)) => *value,
                    Some(_) => {
                        diagnostics.push(
                            ExternalSourceDiagnostic::warning(
                                "lnfu.workspace_reference.hidden_invalid",
                                row_message(
                                    "LNFU workspace reference hidden flag must be a literal boolean",
                                    &declaration.identity,
                                ),
                                Some(source.clone()),
                            )
                            .with_asset_kind(ExternalSourceAssetKind::Reference),
                        );
                        continue;
                    }
                };
                let definition = ExternalWorkspaceReferenceDefinition {
                    source: source.clone(),
                    alias,
                    path: declared,
                    description,
                    hidden,
                    content_version: content_version.clone(),
                };
                if definition.validate().is_err() {
                    diagnostics.push(
                        ExternalSourceDiagnostic::warning(
                            "lnfu.workspace_reference.entry_rejected",
                            row_message(
                                "LNFU workspace reference entry was rejected by the contract validator",
                                &declaration.identity,
                            ),
                            Some(source.clone()),
                        )
                        .with_asset_kind(ExternalSourceAssetKind::Reference),
                    );
                    continue;
                }
                if !aliases.insert(definition.alias.clone()) {
                    diagnostics.push(
                        ExternalSourceDiagnostic::warning(
                            "lnfu.workspace_reference.alias_conflict",
                            row_message(
                                "LNFU workspace reference aliases must be unique across declaration layers",
                                &declaration.identity,
                            ),
                            Some(source.clone()),
                        )
                        .with_asset_kind(ExternalSourceAssetKind::Reference),
                    );
                    continue;
                }
                references.push(definition);
            }
        }

        let snapshot = ExternalWorkspaceReferenceProviderSnapshot {
            provider,
            sources,
            references,
            diagnostics,
        };
        snapshot.validate().map_err(|_| {
            error(
                "snapshot_invalid",
                "LNFU workspace reference snapshot failed contract validation",
                false,
            )
        })?;
        Ok(snapshot)
    }

    fn watch_roots(&self, context: &ExternalSourceContext) -> Vec<ExternalWatchRoot> {
        let mut roots = vec![ExternalWatchRoot {
            path: self.options.lnfu_home.clone(),
            recursive: true,
        }];
        if let Some(path) = &context.workspace_root {
            roots.push(ExternalWatchRoot {
                path: path.join(".lnfu"),
                recursive: false,
            });
        }
        roots
    }
}
