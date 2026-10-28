//! Statically declares the LNFU tool authorization configuration that OpenBitFun
//! must consult before it exposes an LNFU-owned tool. Only literal text is read:
//! no module is evaluated, imported, resolved or launched, and no runtime
//! artifact is produced here.
//!
//! The governance file is `{"tools": [ ... ]}`. Every row that this adapter
//! cannot reduce to literal text stays visible as unsupported instead of being
//! guessed, so a hand-set flag can never widen the model-visible tool surface.
use openbitfun_product_domains::external_sources::{
    EcosystemId, ExternalSourceAssetKind, ExternalSourceContext, ExternalSourceDiagnostic,
    ExternalSourceHealth, ExternalSourceProviderError, ExternalSourceRecord, ExternalSourceScope,
    ExternalToolCapability, ExternalToolDefinition, ExternalToolProviderIdentity,
    ExternalToolProviderSnapshot, ExternalToolRuntimeKind, ExternalToolSourceProvider,
    ExternalToolStaticStatus, ExternalWatchRoot, PreparedExternalToolExport,
    PreparedExternalToolTarget, SourceKey, SourceQualifiedToolId, SourceQualifiedToolTargetId,
};
use openbitfun_static_hook_support::{
    read_bounded_text, resolve_bounded_regular_file, BoundedTextRead,
};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};

const PROVIDER: &str = "lnfu.tools";
const ECOSYSTEM: &str = "lnfu";
const GOVERNANCE_FILE: &str = "authorization.json";
const MAX_BYTES: usize = 1024 * 1024;
const MAX_ROWS: usize = 512;
const MAX_SOURCES: usize = 128;
const MAX_ARGUMENTS: usize = 256;
const MAX_STRING_BYTES: usize = 64 * 1024;
const MAX_NAME_BYTES: usize = 64;
const MODULE_SOURCE_LIMIT: usize = 64 * 1024;

#[derive(Clone, Debug)]
pub struct LnfuToolProviderOptions {
    pub lnfu_home: PathBuf,
}

impl Default for LnfuToolProviderOptions {
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
pub struct LnfuToolProvider {
    options: LnfuToolProviderOptions,
}

/// One governance root plus the scope it contributes.
struct SourceFile {
    path: PathBuf,
    root: PathBuf,
    scope: ExternalSourceScope,
}

/// One literal row of the governance document, kept beside the fields that the
/// document itself did not prove.
#[derive(Clone)]
struct ToolRow {
    identity: String,
    name: String,
    description: String,
    entry: PathBuf,
    arguments: Vec<String>,
    capabilities: Vec<ExternalToolCapability>,
    runtime_kind: ExternalToolRuntimeKind,
    enabled: bool,
    unsupported: Option<String>,
}

impl LnfuToolProvider {
    pub fn new(options: LnfuToolProviderOptions) -> Self {
        Self { options }
    }

    fn files(&self, context: &ExternalSourceContext) -> Vec<SourceFile> {
        let mut files = vec![
            SourceFile {
                path: self.options.lnfu_home.join(GOVERNANCE_FILE),
                root: self.options.lnfu_home.clone(),
                scope: ExternalSourceScope::UserGlobal,
            },
            SourceFile {
                path: self.options.lnfu_home.join("config").join(GOVERNANCE_FILE),
                root: self.options.lnfu_home.clone(),
                scope: ExternalSourceScope::UserGlobal,
            },
        ];
        if let Some(workspace) = &context.workspace_root {
            let root = workspace.join(".lnfu");
            files.push(SourceFile {
                path: root.join(GOVERNANCE_FILE),
                root,
                scope: ExternalSourceScope::Project,
            });
        }
        files
    }

    /// Declares how one literal entry module maps onto the OpenBitFun tool host.
    ///
    /// OpenBitFun loads an external tool by running a module source it already
    /// holds, so this adapter resolves the declared module itself. The declaration
    /// still decides the name, the requested capabilities and whether the row is
    /// enabled at all: preparation never widens what discovery already reported.
    fn target(
        &self,
        row: &ToolRow,
        source: &SourceKey,
    ) -> Result<PreparedExternalToolTarget, ExternalSourceProviderError> {
        let metadata = std::fs::metadata(&row.entry).map_err(|_| {
            error(
                "entry_unavailable",
                "LNFU tool entry is no longer readable",
                true,
            )
        })?;
        if !metadata.is_file() {
            return Err(error(
                "entry_unavailable",
                "LNFU tool entry is not a regular file",
                false,
            ));
        }
        let text = match read_bounded_text(&row.entry, MAX_BYTES) {
            Ok(BoundedTextRead::Content(text)) => text,
            Ok(BoundedTextRead::TooLarge) => {
                return Err(error(
                    "entry_unreadable",
                    "LNFU tool entry exceeds the static discovery size limit",
                    false,
                ))
            }
            Ok(BoundedTextRead::InvalidUtf8) => {
                return Err(error(
                    "entry_unreadable",
                    "LNFU tool entry is not valid UTF-8",
                    false,
                ))
            }
            Err(_) => {
                return Err(error(
                    "entry_unavailable",
                    "Could not read the LNFU tool entry",
                    true,
                ))
            }
        };
        if text.len() > MODULE_SOURCE_LIMIT {
            return Err(error(
                "entry_unreadable",
                "LNFU tool entry exceeds the 64 KiB module limit",
                false,
            ));
        }
        // The target identity is the one discovery published, so it is keyed by
        // the declaration source rather than by the entry the row points at.
        let target_id =
            SourceQualifiedToolTargetId::new(source.clone(), row.identity.clone()).map_err(|_| {
                error(
                    "identity_invalid",
                    "LNFU tool declaration identity is not portable",
                    false,
                )
            })?;
        Ok(PreparedExternalToolTarget {
            target_id,
            content_version: content_version(&row.identity, &declaration_bytes(row)),
            module_source: text,
            module_url: file_url(&row.entry)?,
            working_directory: row
                .entry
                .parent()
                .filter(|parent| !parent.as_os_str().is_empty())
                .map(|parent| parent.to_string_lossy().into_owned())
                .unwrap_or_else(|| ".".into()),
            worktree_root: None,
            expected_tools: vec![PreparedExternalToolExport {
                export_name: "default".into(),
                tool_name: row.name.clone(),
            }],
        })
    }

    fn snapshot(
        &self,
        context: &ExternalSourceContext,
    ) -> Result<ExternalToolProviderSnapshot, ExternalSourceProviderError> {
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
        let mut snapshot = ExternalToolProviderSnapshot {
            provider: self.identity(),
            sources: vec![],
            tools: vec![],
            diagnostics: vec![],
        };
        let mut seen = BTreeSet::new();
        for file in self.files(context) {
            match std::fs::symlink_metadata(&file.path) {
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                Err(_) => {
                    return Err(error(
                        "config_unreadable",
                        "Could not inspect LNFU tool configuration",
                        true,
                    ))
                }
                Ok(_) => {}
            }
            let path = resolve_bounded_regular_file(&file.path, &file.root).map_err(|_| {
                error(
                    "config_unreadable",
                    "LNFU tool configuration must resolve to a regular file inside its source root",
                    false,
                )
            })?;
            if !seen.insert(path.clone()) {
                continue;
            }
            if snapshot.sources.len() >= MAX_SOURCES {
                return Err(error(
                    "source_limit",
                    "LNFU tool source limit reached",
                    false,
                ));
            }
            let text = match read_bounded_text(&path, MAX_BYTES) {
                Ok(BoundedTextRead::Content(text)) => text,
                _ => {
                    return Err(error(
                        "config_unreadable",
                        "LNFU tool configuration must be readable UTF-8 within 1 MiB",
                        false,
                    ))
                }
            };
            let key = source_key(&path);
            let value: Value = serde_json::from_str(&text).map_err(|_| {
                error(
                    "config_invalid",
                    "LNFU tool configuration is not valid JSON",
                    false,
                )
            })?;
            let (rows, structural_reason) = collect_rows(&value, &self.options.lnfu_home)?;
            let mut diagnostics = Vec::new();
            if let Some(reason) = &structural_reason {
                diagnostics.push(diagnostic(&key, "declaration_incomplete", reason));
            }
            if !rows.is_empty() {
                diagnostics.push(diagnostic(
                    &key,
                    "declaration_scope",
                    "Explicit LNFU tool authorizations are reused independently in OpenBitFun; LNFU profile selection, strategy-bundle overlays and reconnect lifecycle are not imported",
                ));
            }
            let mut declared_names = BTreeSet::new();
            let duplicate_name = rows.iter().any(|row| {
                !declared_names.insert(row.name.to_ascii_lowercase())
            });
            let mut supported = 0usize;
            for row in rows.into_iter().filter(|row| row.enabled) {
                if snapshot.tools.len() >= MAX_ROWS {
                    return Err(error("server_limit", "LNFU tool limit reached", false));
                }
                let mut status = ExternalToolStaticStatus::Ready;
                if let Some(reason) = row.unsupported.clone().or_else(|| {
                    duplicate_name
                        .then(|| "LNFU tool names must be unique ignoring case".to_string())
                }) {
                    status = ExternalToolStaticStatus::Unsupported { reason };
                } else if let Some(reason) = container_reason(&structural_reason, &row) {
                    status = ExternalToolStaticStatus::Unsupported { reason };
                }
                let export_id = digest(row.identity.as_bytes());
                snapshot.tools.push(ExternalToolDefinition {
                    id: SourceQualifiedToolId::new(
                        SourceQualifiedToolTargetId::new(key.clone(), row.identity.clone())
                            .map_err(|_| {
                                error(
                                    "identity_invalid",
                                    "LNFU tool declaration identity is not portable",
                                    false,
                                )
                            })?,
                        export_id,
                    )
                    .map_err(|_| {
                        error(
                            "identity_invalid",
                            "LNFU tool declaration identity is not portable",
                            false,
                        )
                    })?,
                    name: row.name.clone(),
                    description_preview: row.description.clone(),
                    module_path: display_path(&path, &row.entry),
                    // The contract requires a non-empty working directory even
                    // for a row that is not executable, so an unresolved entry
                    // falls back to the configuration root rather than an empty
                    // string that would invalidate the whole snapshot.
                    working_directory: row
                        .entry
                        .parent()
                        .filter(|parent| !parent.as_os_str().is_empty())
                        .map(|parent| parent.to_string_lossy().into_owned())
                        .unwrap_or_else(|| {
                            path.parent()
                                .map(|parent| parent.to_string_lossy().into_owned())
                                .unwrap_or_else(|| ".".into())
                        }),
                    runtime_kind: row.runtime_kind,
                    capabilities: row.capabilities.clone(),
                    // The published version must move when the declaration
                    // moves, so it covers the literal row text as well as the
                    // entry bytes; otherwise an edited declaration would keep a
                    // previously approved revision alive.
                    content_version: content_version(&row.identity, &declaration_bytes(&row)),
                    static_status: status.clone(),
                });
                // Only a row whose literal text is fully understood is offered to
                // the activation path; anything downgraded above is not.
                if matches!(status, ExternalToolStaticStatus::Ready) {
                    supported += 1;
                }
            }
            snapshot.sources.push(ExternalSourceRecord {
                key: key.clone(),
                ecosystem_id: EcosystemId::new(ECOSYSTEM).expect("static id"),
                display_name: "LNFU tool authorization".into(),
                source_kind: "lnfu_tool_config".into(),
                scope: file.scope,
                location: text_location(&path, &text),
                execution_domain_id: context.execution_domain_id.clone(),
                health: if structural_reason.is_some() || duplicate_name {
                    ExternalSourceHealth::Degraded
                } else if supported == 0 {
                    ExternalSourceHealth::Partial
                } else {
                    ExternalSourceHealth::Available
                },
                content_version: content_version(&key.stable_key(), text.as_bytes()),
                diagnostics: diagnostics.clone(),
            });
            snapshot.diagnostics.extend(diagnostics);
        }
        snapshot.validate().map_err(|_| {
            error(
                "snapshot_invalid",
                "LNFU tool catalog could not be validated",
                false,
            )
        })?;
        Ok(snapshot)
    }
}

impl ExternalToolSourceProvider for LnfuToolProvider {
    fn identity(&self) -> ExternalToolProviderIdentity {
        ExternalToolProviderIdentity::new(PROVIDER, ECOSYSTEM, "LNFU").expect("static id")
    }

    fn discover(
        &self,
        context: &ExternalSourceContext,
    ) -> Result<ExternalToolProviderSnapshot, ExternalSourceProviderError> {
        self.snapshot(context)
    }

    fn prepare_target(
        &self,
        context: &ExternalSourceContext,
        target_id: &SourceQualifiedToolTargetId,
        expected_content_version: &str,
    ) -> Result<PreparedExternalToolTarget, ExternalSourceProviderError> {
        if target_id.source.provider_id.as_str() != PROVIDER {
            return Err(error(
                "identity_mismatch",
                "Tool target is not owned by LNFU",
                false,
            ));
        }
        let snapshot = self.snapshot(context)?;
        let definition = snapshot
            .tools
            .iter()
            .find(|definition| &definition.id.target == target_id)
            .ok_or_else(|| {
                error(
                    "stale_revision",
                    "LNFU tool declaration is no longer available",
                    true,
                )
            })?;
        if definition.content_version != expected_content_version {
            return Err(error(
                "stale_revision",
                "LNFU tool declaration changed before preparation",
                true,
            ));
        }
        if !matches!(definition.static_status, ExternalToolStaticStatus::Ready) {
            return Err(error(
                "not_activatable",
                "LNFU tool declaration is disabled or unsupported",
                false,
            ));
        }
        // The row is re-read here rather than carried over, so preparation can
        // never widen the surface that discovery already published.
        let (rows, _) = self.current_rows(context)?;
        let row = rows
            .into_iter()
            .find(|row| row.identity == target_id.local_id.as_str() && row.enabled)
            .ok_or_else(|| {
                error(
                    "stale_revision",
                    "LNFU tool declaration is no longer available",
                    true,
                )
            })?;
        if row.unsupported.is_some() {
            return Err(error(
                "not_activatable",
                "LNFU tool declaration is not fully supported",
                false,
            ));
        }
        self.target(&row, &target_id.source)
    }

    fn watch_roots(&self, context: &ExternalSourceContext) -> Vec<ExternalWatchRoot> {
        let mut roots = vec![ExternalWatchRoot {
            path: self.options.lnfu_home.clone(),
            recursive: true,
        }];
        if let Some(path) = &context.workspace_root {
            roots.push(ExternalWatchRoot {
                path: path.clone(),
                recursive: false,
            });
        }
        roots
    }
}

impl LnfuToolProvider {
    /// Re-derives every row that the current configuration literally declares.
    fn current_rows(
        &self,
        context: &ExternalSourceContext,
    ) -> Result<(Vec<ToolRow>, bool), ExternalSourceProviderError> {
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
        let mut rows = Vec::new();
        let mut duplicate = false;
        let mut seen = BTreeSet::new();
        for file in self.files(context) {
            match std::fs::symlink_metadata(&file.path) {
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                Err(_) => {
                    return Err(error(
                        "config_unreadable",
                        "Could not inspect LNFU tool configuration",
                        true,
                    ))
                }
                Ok(_) => {}
            }
            let path = resolve_bounded_regular_file(&file.path, &file.root).map_err(|_| {
                error(
                    "config_unreadable",
                    "LNFU tool configuration must resolve to a regular file inside its source root",
                    false,
                )
            })?;
            if !seen.insert(path.clone()) {
                continue;
            }
            let text = match read_bounded_text(&path, MAX_BYTES) {
                Ok(BoundedTextRead::Content(text)) => text,
                _ => {
                    return Err(error(
                        "config_unreadable",
                        "LNFU tool configuration must be readable UTF-8 within 1 MiB",
                        false,
                    ))
                }
            };
            let value: Value = serde_json::from_str(&text).map_err(|_| {
                error(
                    "config_invalid",
                    "LNFU tool configuration is not valid JSON",
                    false,
                )
            })?;
            let (declared, _) = collect_rows(&value, &self.options.lnfu_home)?;
            let mut names = BTreeSet::new();
            duplicate |= declared
                .iter()
                .any(|row| !names.insert(row.name.to_ascii_lowercase()));
            rows.extend(declared);
        }
        Ok((rows, duplicate))
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

/// Extracts literal tool rows from one LNFU authorization document.
///
/// The second value is set when the container itself is shaped in a way that
/// prevents the literal rows from being trusted as-is.
fn collect_rows(
    value: &Value,
    home: &Path,
) -> Result<(Vec<ToolRow>, Option<String>), ExternalSourceProviderError> {
    let Some(root) = value.as_object() else {
        return Err(error(
            "config_invalid",
            "LNFU tool configuration must be a JSON object",
            false,
        ));
    };
    if root.keys().any(|key| !matches!(key.as_str(), "tools" | "version")) {
        return Err(error(
            "config_invalid",
            "LNFU tool configuration contains unsupported top-level fields",
            false,
        ));
    }
    let rows = match root.get("tools") {
        None => return Ok((Vec::new(), None)),
        Some(Value::Array(rows)) => rows,
        Some(_) => {
            return Err(error(
                "config_invalid",
                "LNFU tool configuration 'tools' must be an array",
                false,
            ))
        }
    };
    if rows.len() > MAX_ROWS {
        return Err(error("server_limit", "LNFU tool limit reached", false));
    }
    let mut collected = Vec::with_capacity(rows.len());
    let mut container_reason = None;
    for (index, raw) in rows.iter().enumerate() {
        let identity = format!("/tools/{index}");
        let Some(row) = raw.as_object() else {
            collected.push(invalid_row(identity));
            continue;
        };
        collected.push(normalize_row(identity, row, home, &mut container_reason));
    }
    Ok((collected, container_reason))
}

fn invalid_row(identity: String) -> ToolRow {
    // Row-local ids must stay portable identifiers even when the row is
    // malformed, because the id is what makes the row addressable at all.
    // The name is taken from the identity before the identity is moved.
    let name = identity.clone();
    ToolRow {
        identity,
        name,
        description: String::new(),
        entry: PathBuf::new(),
        arguments: vec![],
        capabilities: vec![],
        runtime_kind: ExternalToolRuntimeKind::JavaScript,
        enabled: false,
        unsupported: Some("LNFU tool entry must be a JSON object".into()),
    }
}

/// Validates one row and reduces it to the literal fields this adapter
/// understands. Unknown or indirect fields produce a reason; nothing is
/// substituted with a placeholder value.
fn normalize_row(
    identity: String,
    row: &Map<String, Value>,
    home: &Path,
    container_reason: &mut Option<String>,
) -> ToolRow {
    let mut reason: Option<String> = None;
    const ALLOWED: &[&str] = &[
        "name",
        "entry",
        "description",
        "arguments",
        "capabilities",
        "enabled",
    ];
    if row.keys().any(|key| !ALLOWED.contains(&key.as_str())) {
        note(
            &mut reason,
            "LNFU tool configuration has unsupported fields; the effective tool cannot be derived from literal text",
        );
    }
    let raw_name = match row.get("name") {
        Some(Value::String(name)) => name.clone(),
        Some(_) => {
            note(&mut reason, "LNFU tool name must be a string");
            String::new()
        }
        None => {
            note(&mut reason, "LNFU tool entry must declare a literal 'name'");
            String::new()
        }
    };
    // The portable name is what the model is allowed to call. A declaration that
    // asks for a wider name is reported instead of being silently truncated.
    if !valid_name(&raw_name) {
        if !raw_name.is_empty() {
            note(
                &mut reason,
                "LNFU tool name must contain 1 to 64 ASCII letters, numbers, underscores or hyphens",
            );
        } else if reason.is_none() {
            note(&mut reason, "LNFU tool entry must declare a literal 'name'");
        }
    }
    let description = match row.get("description") {
        None => String::new(),
        Some(value) => match bounded_string(value) {
            Some(text) => text.to_string(),
            None => {
                note(
                    &mut reason,
                    "LNFU tool 'description' must be a bounded literal string",
                );
                String::new()
            }
        },
    };
    let entry = match row.get("entry") {
        Some(value) => match bounded_string(value).and_then(|text| {
            if text.contains(['$', '\0']) {
                None
            } else {
                Some(expand_path(text, home))
            }
        }) {
            Some(path) => path,
            None => {
                note(
                    &mut reason,
                    "LNFU tool 'entry' must be a literal path without indirection",
                );
                PathBuf::new()
            }
        },
        None => {
            note(
                &mut reason,
                "LNFU tool entry must declare a literal 'entry'",
            );
            PathBuf::new()
        }
    };
    if !entry.as_os_str().is_empty() && !entry.is_absolute() {
        note(
            &mut reason,
            "LNFU tool 'entry' must be an absolute path or start with ~/",
        );
    }
    let arguments = match row.get("arguments") {
        None => vec![],
        Some(Value::Array(items)) if items.len() <= MAX_ARGUMENTS => {
            let mut arguments = Vec::with_capacity(items.len());
            for item in items {
                match bounded_string(item) {
                    Some(text) => arguments.push(text.to_string()),
                    None => {
                        note(
                            &mut reason,
                            format!(
                                "LNFU tool 'arguments' must be an array of at most {MAX_ARGUMENTS} literal strings"
                            ),
                        );
                        break;
                    }
                }
            }
            arguments
        }
        Some(_) => {
            note(
                &mut reason,
                format!(
                    "LNFU tool 'arguments' must be an array of at most {MAX_ARGUMENTS} literal strings"
                ),
            );
            vec![]
        }
    };
    let mut capabilities = Vec::new();
    match row.get("capabilities") {
        None => {}
        Some(Value::Array(items)) => {
            for item in items {
                match item.as_str().and_then(capability) {
                    Some(capability) if !capabilities.contains(&capability) => {
                        capabilities.push(capability)
                    }
                    Some(_) => {}
                    None => {
                        note(
                            &mut reason,
                            "LNFU tool 'capabilities' must name only file_system, network, process or environment",
                        );
                        break;
                    }
                }
            }
        }
        Some(_) => note(
            &mut reason,
            "LNFU tool 'capabilities' must be an array of literal strings",
        ),
    }
    // Only literal JavaScript is executable through the external tool host, so a
    // declared TypeScript entry is disclosed rather than misrepresented.
    let runtime_kind = match entry.extension().and_then(|value| value.to_str()) {
        Some("ts") => ExternalToolRuntimeKind::TypeScript,
        Some("js") => ExternalToolRuntimeKind::JavaScript,
        // No usable extension: an absent entry is a missing-field problem that
        // the caller already reported, while any other path shape makes the
        // row non-executable and is disclosed here.
        None if entry.as_os_str().is_empty() => ExternalToolRuntimeKind::JavaScript,
        None => {
            note(
                &mut reason,
                "LNFU tool 'entry' must be a .js or .ts file",
            );
            ExternalToolRuntimeKind::JavaScript
        }
        Some(_) => {
            note(
                &mut reason,
                "LNFU tool 'entry' must be a .js or .ts file",
            );
            ExternalToolRuntimeKind::JavaScript
        }
    };
    if runtime_kind == ExternalToolRuntimeKind::TypeScript {
        note(
            &mut reason,
            "LNFU TypeScript tools are recognized but are not executable yet",
        );
    }
    let mut enabled = true;
    if let Some(value) = row.get("enabled") {
        match value.as_bool() {
            Some(declared) => enabled = declared,
            None => note(&mut reason, "LNFU tool 'enabled' must be a literal boolean"),
        }
    }
    if container_reason.is_some() {
        note(
            &mut reason,
            "LNFU tool configuration container is not a plain literal declaration list, so row semantics are not trusted",
        );
    }
    ToolRow {
        identity,
        name: raw_name,
        description,
        entry,
        arguments,
        capabilities,
        runtime_kind,
        enabled,
        unsupported: reason,
    }
}

fn container_reason(
    structural_reason: &Option<String>,
    _row: &ToolRow,
) -> Option<String> {
    structural_reason.clone()
}

fn note(reason: &mut Option<String>, message: impl Into<String>) {
    reason.get_or_insert_with(|| message.into());
}

fn capability(value: &str) -> Option<ExternalToolCapability> {
    match value {
        "file_system" => Some(ExternalToolCapability::FileSystem),
        "network" => Some(ExternalToolCapability::Network),
        "process" => Some(ExternalToolCapability::Process),
        "environment" => Some(ExternalToolCapability::Environment),
        _ => None,
    }
}

fn bounded_string(value: &Value) -> Option<&str> {
    value
        .as_str()
        .filter(|text| text.len() <= MAX_STRING_BYTES && !text.contains('\0'))
}

fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_NAME_BYTES
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
}

/// Fingerprints the literal declaration of one row. Declared arguments are part
/// of the digest input but never of the digest output, so the revision moves when
/// a declaration is edited while the arguments themselves stay undisclosed.
fn declaration_bytes(row: &ToolRow) -> Vec<u8> {
    let mut bytes = Vec::new();
    for field in [
        row.name.as_str(),
        row.description.as_str(),
        row.entry.to_string_lossy().as_ref(),
        row.runtime_kind.as_str(),
    ] {
        bytes.extend_from_slice(field.as_bytes());
        bytes.push(0);
    }
    for capability in &row.capabilities {
        bytes.extend_from_slice(capability.as_str().as_bytes());
        bytes.push(0);
    }
    // The argument list length and each argument take part in the digest so that
    // a changed argument yields a new revision without being recoverable from it.
    bytes.extend_from_slice(&(row.arguments.len() as u64).to_be_bytes());
    for argument in &row.arguments {
        bytes.extend_from_slice(argument.as_bytes());
        bytes.push(0);
    }
    bytes
}

/// Derives a revision from facts the catalog already exposes. The argument list
/// is not disclosed here, and no credential can influence the public version.
fn content_version(identity: &str, bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"lnfu.tool.content.v1");
    hasher.update([0]);
    hasher.update(identity.as_bytes());
    hasher.update([0]);
    hasher.update((bytes.len() as u64).to_be_bytes());
    hasher.update(bytes);
    format!("sha256:{}", hex::encode(hasher.finalize()))
}

fn source_key(path: &Path) -> SourceKey {
    SourceKey::new(
        PROVIDER,
        format!("lnfu_tool-{}", digest(path.to_string_lossy().as_bytes())),
    )
    .expect("hashed id")
}

fn display_path(config: &Path, entry: &Path) -> String {
    match entry.strip_prefix(config.parent().unwrap_or(config)) {
        Ok(relative) => format!("<lnfu>/{GOVERNANCE_FILE}:{}", relative.to_string_lossy()),
        Err(_) => format!("<lnfu>/{GOVERNANCE_FILE}:#entry"),
    }
}

/// Records where the declaration was read without echoing its text. The raw
/// authorization document carries declared arguments, so the location keeps the
/// path and a content digest only; nothing a declaration wrote can reach a
/// diagnostic or a remote surface through this field.
fn text_location(path: &Path, text: &str) -> String {
    let location = path.to_string_lossy();
    format!("{location}#sha256:{}", digest(text.as_bytes()))
}

fn file_url(path: &Path) -> Result<String, ExternalSourceProviderError> {
    let normalized = path.to_string_lossy().replace('\\', "/");
    let encoded = normalized
        .chars()
        .map(|character| match character {
            ' ' => "%20".to_string(),
            '#' => "%23".to_string(),
            '?' => "%3F".to_string(),
            '%' => "%25".to_string(),
            ':' | '/' | '-' | '_' | '.' | '~' => character.to_string(),
            value if value.is_ascii_alphanumeric() => value.to_string(),
            value => {
                let mut buffer = [0u8; 4];
                value.encode_utf8(&mut buffer).as_bytes().iter().fold(
                    String::new(),
                    |mut encoded, byte| {
                        encoded.push_str(&format!("%{byte:02X}"));
                        encoded
                    },
                )
            }
        })
        .collect::<String>();
    let url = if normalized.starts_with('/') {
        format!("file://{encoded}")
    } else {
        format!("file:///{encoded}")
    };
    if url.len() > 4096 {
        return Err(error(
            "module_url_invalid",
            "LNFU tool entry path is too long to become a module URL",
            false,
        ));
    }
    Ok(url)
}

fn digest(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))[..24].to_string()
}

fn error(suffix: &str, message: &str, transient: bool) -> ExternalSourceProviderError {
    ExternalSourceProviderError::new(format!("lnfu.tool.{suffix}"), message, transient)
}

fn diagnostic(source: &SourceKey, suffix: &str, message: &str) -> ExternalSourceDiagnostic {
    ExternalSourceDiagnostic::warning(format!("lnfu.tool.{suffix}"), message, Some(source.clone()))
        .with_asset_kind(ExternalSourceAssetKind::Tool)
}

#[cfg(test)]
mod tests {
    use super::*;
    use openbitfun_product_domains::external_sources::ExecutionDomainId;

    fn input(workspace: Option<PathBuf>) -> ExternalSourceContext {
        ExternalSourceContext {
            workspace_root: workspace,
            execution_domain_id: ExecutionDomainId::new("fixture").unwrap(),
        }
    }

    fn write(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    fn provider(home: &Path) -> LnfuToolProvider {
        LnfuToolProvider::new(LnfuToolProviderOptions {
            lnfu_home: home.to_path_buf(),
        })
    }

    #[test]
    fn discovers_literal_tool_authorizations_without_disclosing_arguments() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        write(&home.join("run_echo.js"), "export default {};\n");
        write(
            &home.join(GOVERNANCE_FILE),
            r#"{"tools":[
                {"name":"lnfu_echo","entry":"~/run_echo.js","arguments":["SECRET"],"capabilities":["process"],"description":"Echo one argument"},
                {"name":"lnfu_disabled","entry":"~/run_echo.js","enabled":false}
            ]}"#,
        );
        let snapshot = provider(&home).discover(&input(None)).unwrap();
        assert_eq!(snapshot.provider.ecosystem_id.as_str(), "lnfu");
        assert_eq!(snapshot.sources.len(), 1);
        assert_eq!(snapshot.sources[0].source_kind, "lnfu_tool_config");
        assert_eq!(snapshot.tools.len(), 1);
        let tool = &snapshot.tools[0];
        assert_eq!(tool.name, "lnfu_echo");
        assert_eq!(tool.runtime_kind, ExternalToolRuntimeKind::JavaScript);
        assert_eq!(tool.capabilities, vec![ExternalToolCapability::Process]);
        assert_eq!(tool.static_status, ExternalToolStaticStatus::Ready);
        assert_eq!(tool.description_preview, "Echo one argument");
        let encoded = serde_json::to_string(&snapshot).unwrap();
        assert!(!encoded.contains("SECRET"));
        assert!(!encoded.contains(temp.path().to_string_lossy().as_ref()));
    }

    #[test]
    fn indirect_or_unknown_rows_stay_unsupported_without_placeholders() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        write(
            &home.join(GOVERNANCE_FILE),
            r#"{"tools":[
                {"name":"lnfu_indirect","entry":"${LNFU_TOOL}"},
                {"name":"lnfu_relative","entry":"tools/echo.js"},
                {"name":"lnfu_typed","entry":"~/echo.ts","capabilities":["shell"]}
            ]}"#,
        );
        let snapshot = provider(&home).discover(&input(None)).unwrap();
        assert_eq!(snapshot.tools.len(), 3);
        assert!(snapshot
            .tools
            .iter()
            .all(|tool| matches!(
                tool.static_status,
                ExternalToolStaticStatus::Unsupported { .. }
            )));
        assert_eq!(snapshot.sources[0].health, ExternalSourceHealth::Partial);
        assert!(snapshot
            .sources
            .iter()
            .flat_map(|source| &source.diagnostics)
            .any(|diagnostic| diagnostic.code == "lnfu.tool.declaration_scope"));
    }

    #[test]
    fn prepare_target_is_guarded_by_the_published_content_version() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        write(&home.join("run_echo.js"), "export default { name: 'echo' };\n");
        write(
            &home.join(GOVERNANCE_FILE),
            r#"{"tools":[{"name":"lnfu_echo","entry":"~/run_echo.js","arguments":["SECRET"]}]}"#,
        );
        let context = input(None);
        let provider = provider(&home);
        let snapshot = provider.discover(&context).unwrap();
        let definition = &snapshot.tools[0];
        let prepared = provider
            .prepare_target(
                &context,
                &definition.id.target,
                &definition.content_version,
            )
            .unwrap();
        assert_eq!(prepared.target_id, definition.id.target);
        assert_eq!(prepared.expected_tools.len(), 1);
        assert_eq!(prepared.expected_tools[0].tool_name, "lnfu_echo");
        assert_eq!(prepared.module_source, "export default { name: 'echo' };\n");
        assert!(prepared.module_url.starts_with("file:"));
        assert!(provider
            .prepare_target(&context, &definition.id.target, "sha256:stale")
            .is_err());
        write(
            &home.join(GOVERNANCE_FILE),
            r#"{"tools":[{"name":"lnfu_echo","entry":"~/run_echo.js","arguments":["OTHER"]}]}"#,
        );
        assert!(provider
            .prepare_target(
                &context,
                &definition.id.target,
                &definition.content_version
            )
            .is_err());
    }

    #[test]
    fn unresolvable_rows_are_never_prepared() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        write(
            &home.join(GOVERNANCE_FILE),
            r#"{"tools":[{"name":"lnfu_relative","entry":"tools/echo.js"}]}"#,
        );
        let context = input(None);
        let provider = provider(&home);
        let snapshot = provider.discover(&context).unwrap();
        let definition = &snapshot.tools[0];
        assert!(provider
            .prepare_target(
                &context,
                &definition.id.target,
                &definition.content_version
            )
            .is_err());
    }

    #[test]
    fn watch_roots_cover_lnfu_home_and_the_selected_workspace() {
        let provider = LnfuToolProvider::new(LnfuToolProviderOptions {
            lnfu_home: PathBuf::from("C:/lnfu-home"),
        });
        let workspace = PathBuf::from("C:/workspace");
        let roots = provider.watch_roots(&input(Some(workspace.clone())));
        assert_eq!(roots.len(), 2);
        assert_eq!(roots[0].path, PathBuf::from("C:/lnfu-home"));
        assert!(roots[0].recursive);
        assert_eq!(roots[1].path, workspace);
        assert!(!roots[1].recursive);
    }

    #[test]
    fn project_configuration_outside_its_root_is_rejected() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        let workspace = temp.path().join("workspace");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(&workspace).unwrap();
        write(&workspace.join(GOVERNANCE_FILE), r#"{"tools":[]}"#);
        let provider = provider(&home);
        assert!(provider
            .discover(&input(Some(workspace.clone())))
            .unwrap()
            .sources
            .is_empty());
        write(&home.join(GOVERNANCE_FILE), "not json");
        let error = provider.discover(&input(Some(workspace))).unwrap_err();
        assert_eq!(error.code, "lnfu.tool.config_invalid");
    }
}
