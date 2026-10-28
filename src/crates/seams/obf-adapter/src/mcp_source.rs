//! Reuses explicit LNFU MCP server declarations without launching the LNFU
//! runtime. Each declaration file is an independent source; entries whose
//! effective configuration cannot be derived from literal text stay visible as
//! unsupported instead of being guessed.
use openbitfun_product_domains::external_sources::*;
use openbitfun_static_hook_support::{
    read_bounded_text, redacted_executable_preview, resolve_bounded_regular_file, BoundedTextRead,
};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

const PROVIDER: &str = "lnfu.mcp";
const ECOSYSTEM: &str = "lnfu";
const MAX_BYTES: usize = 1024 * 1024;
const MAX_FILES: usize = 128;
const MAX_SERVERS: usize = 256;
const DEFAULT_CALL_TIMEOUT: u64 = 60_000;

#[derive(Clone, Debug)]
pub struct LnfuMcpProviderOptions {
    pub lnfu_home: PathBuf,
}

impl Default for LnfuMcpProviderOptions {
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
pub struct LnfuMcpProvider {
    options: LnfuMcpProviderOptions,
}

struct SourceFile {
    path: PathBuf,
    root: PathBuf,
    scope: ExternalSourceScope,
}

struct Materialized {
    snapshot: ExternalMcpProviderSnapshot,
    transports: BTreeMap<String, PreparedExternalMcpTransport>,
}

#[derive(Clone)]
struct Declaration {
    identity: String,
    config: Value,
    disabled: bool,
    unsupported: Option<String>,
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

impl LnfuMcpProvider {
    pub fn new(options: LnfuMcpProviderOptions) -> Self {
        Self { options }
    }

    fn files(
        &self,
        context: &ExternalSourceContext,
    ) -> Result<Vec<SourceFile>, ExternalSourceProviderError> {
        let mut files = vec![
            SourceFile {
                path: self.options.lnfu_home.join("mcp.json"),
                root: self.options.lnfu_home.clone(),
                scope: ExternalSourceScope::UserGlobal,
            },
            SourceFile {
                path: self.options.lnfu_home.join("config/mcp.json"),
                root: self.options.lnfu_home.clone(),
                scope: ExternalSourceScope::UserGlobal,
            },
        ];
        if let Some(workspace) = &context.workspace_root {
            let root = workspace.join(".lnfu");
            files.push(SourceFile {
                path: root.join("mcp.json"),
                root,
                scope: ExternalSourceScope::Project,
            });
        }
        if files.len() > MAX_FILES {
            return Err(error(
                "source_limit",
                "LNFU MCP declaration source limit reached",
                false,
            ));
        }
        files.truncate(MAX_FILES);
        Ok(files)
    }

    fn materialize(
        &self,
        input: &ExternalMcpDiscoveryInput,
    ) -> Result<Materialized, ExternalSourceProviderError> {
        if !self.options.lnfu_home.is_absolute()
            || input
                .context
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
        let mut snapshot = ExternalMcpProviderSnapshot {
            provider: self.identity(),
            sources: vec![],
            servers: vec![],
            diagnostics: vec![],
        };
        let mut transports = BTreeMap::new();
        let mut seen = BTreeSet::new();
        for file in self.files(&input.context)? {
            match std::fs::symlink_metadata(&file.path) {
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                Err(_) => {
                    return Err(error(
                        "config_unreadable",
                        "Could not inspect LNFU MCP configuration",
                        true,
                    ))
                }
                Ok(_) => {}
            }
            let path = resolve_bounded_regular_file(&file.path, &file.root).map_err(|_| {
                error(
                    "config_unreadable",
                    "LNFU MCP source must resolve to a regular file inside its source root",
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
                        "LNFU MCP source must be readable UTF-8 within 1 MiB",
                        false,
                    ))
                }
            };
            let key = source_key(&path);
            let suppressed = input.suppressed_sources.contains(&key);
            let value: Value = serde_json::from_str(&text)
                .map_err(|_| error("config_invalid", "LNFU MCP source is not valid JSON", false))?;
            let (declarations, structural_reason) = collect_rows(&value)?;
            let mut diagnostics = Vec::new();
            if let Some(reason) = &structural_reason {
                diagnostics.push(diagnostic(&key, "declaration_incomplete", reason));
            }
            if !declarations.is_empty() {
                diagnostics.push(diagnostic(
                    &key,
                    "declaration_scope",
                    "Explicit LNFU MCP declarations are reused independently in OpenBitFun; LNFU profile selection, strategy-bundle overlays and reconnect lifecycle are not imported",
                ));
            }
            // Declaration identities are positional, so only the literal server
            // name can collide. A collision means the public tool namespace is
            // ambiguous, so no row is trusted as ready.
            let mut declared_names = BTreeSet::new();
            let duplicate_name = declarations.iter().any(|declaration| {
                declaration
                    .config
                    .get("name")
                    .and_then(Value::as_str)
                    .is_some_and(|name| !declared_names.insert(name.to_ascii_lowercase()))
            });
            for mut declaration in declarations.into_iter().filter(|_| !suppressed) {
                if snapshot.servers.len() >= MAX_SERVERS {
                    return Err(error(
                        "server_limit",
                        "LNFU MCP server limit reached",
                        false,
                    ));
                }
                if let Some(reason) = &structural_reason {
                    declaration.unsupported = Some(reason.clone());
                }
                if duplicate_name {
                    declaration.unsupported = Some(
                        "LNFU MCP declaration server names must be unique ignoring case".into(),
                    );
                }
                let (definition, transport) = materialize_server(input, &key, declaration)?;
                transports.insert(definition.id.stable_key(), transport);
                snapshot.servers.push(definition);
            }
            snapshot.sources.push(ExternalSourceRecord {
                key: key.clone(),
                ecosystem_id: EcosystemId::new(ECOSYSTEM).expect("static id"),
                display_name: "LNFU MCP declarations".into(),
                source_kind: "lnfu_mcp_config".into(),
                scope: file.scope,
                location: path.to_string_lossy().into_owned(),
                execution_domain_id: input.context.execution_domain_id.clone(),
                health: if structural_reason.is_some() || duplicate_name {
                    ExternalSourceHealth::Degraded
                } else {
                    ExternalSourceHealth::Available
                },
                content_version: input
                    .revision_key
                    .opaque_revision("lnfu.mcp.content.v1", [text.as_bytes()]),
                diagnostics: diagnostics.clone(),
            });
            snapshot.diagnostics.extend(diagnostics);
        }
        snapshot.validate().map_err(|_| {
            error(
                "snapshot_invalid",
                "LNFU MCP catalog could not be validated",
                false,
            )
        })?;
        Ok(Materialized {
            snapshot,
            transports,
        })
    }

    fn current(
        &self,
        input: &ExternalMcpDiscoveryInput,
        id: &SourceQualifiedMcpServerId,
        version: &str,
    ) -> Result<
        (ExternalMcpServerDefinition, PreparedExternalMcpTransport),
        ExternalSourceProviderError,
    > {
        if id.source.provider_id.as_str() != PROVIDER {
            return Err(error(
                "identity_mismatch",
                "MCP server is not owned by LNFU",
                false,
            ));
        }
        let mut materialized = self.materialize(input)?;
        let definition = materialized
            .snapshot
            .servers
            .into_iter()
            .find(|definition| &definition.id == id)
            .ok_or_else(|| {
                error(
                    "stale_revision",
                    "LNFU MCP declaration is no longer available",
                    true,
                )
            })?;
        if definition.behavior_version != version {
            return Err(error(
                "stale_revision",
                "LNFU MCP declaration changed before preparation",
                true,
            ));
        }
        if !definition.source_enabled
            || !matches!(definition.static_status, ExternalMcpStaticStatus::Ready)
        {
            return Err(error(
                "not_activatable",
                "LNFU MCP declaration is disabled or unsupported",
                false,
            ));
        }
        let transport = materialized
            .transports
            .remove(&id.stable_key())
            .ok_or_else(|| {
                error(
                    "preparation_missing",
                    "LNFU MCP preparation is unavailable",
                    false,
                )
            })?;
        Ok((definition, transport))
    }
}

impl ExternalMcpSourceProvider for LnfuMcpProvider {
    fn identity(&self) -> ExternalMcpProviderIdentity {
        ExternalMcpProviderIdentity::new(PROVIDER, ECOSYSTEM, "LNFU").expect("static id")
    }

    fn discover(
        &self,
        input: &ExternalMcpDiscoveryInput,
    ) -> Result<ExternalMcpProviderSnapshot, ExternalSourceProviderError> {
        self.materialize(input)
            .map(|materialized| materialized.snapshot)
    }

    fn prepare_server(
        &self,
        input: &ExternalMcpDiscoveryInput,
        id: &SourceQualifiedMcpServerId,
        version: &str,
    ) -> Result<PreparedExternalMcpServer, ExternalSourceProviderError> {
        let (definition, transport) = self.current(input, id, version)?;
        Ok(PreparedExternalMcpServer {
            id: id.clone(),
            behavior_version: version.into(),
            timeouts: definition.timeouts,
            transport,
        })
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

/// Extracts literal server declarations from one LNFU MCP document.
///
/// The document is `{"servers": [ ... ]}`. A second `unsupported` reason is
/// returned when the container itself is shaped in a way that prevents the
/// literal rows from being trusted as-is.
fn collect_rows(
    value: &Value,
) -> Result<(Vec<Declaration>, Option<String>), ExternalSourceProviderError> {
    let Some(root) = value.as_object() else {
        return Err(error(
            "config_invalid",
            "LNFU MCP document must be a JSON object",
            false,
        ));
    };
    if root
        .keys()
        .any(|key| !matches!(key.as_str(), "servers" | "version"))
    {
        return Err(error(
            "config_invalid",
            "LNFU MCP document contains unsupported top-level fields",
            false,
        ));
    }
    let rows = match root.get("servers") {
        None => return Ok((Vec::new(), None)),
        Some(Value::Array(rows)) => rows,
        Some(_) => {
            return Err(error(
                "config_invalid",
                "LNFU MCP 'servers' must be an array",
                false,
            ))
        }
    };
    if rows.len() > MAX_SERVERS {
        return Err(error(
            "server_limit",
            "LNFU MCP server limit reached",
            false,
        ));
    }
    let mut declarations = Vec::with_capacity(rows.len());
    let mut container_reason = None;
    for (index, raw) in rows.iter().enumerate() {
        let identity = format!("/servers/{index}");
        let Some(row) = raw.as_object() else {
            declarations.push(Declaration {
                identity,
                config: Value::Object(Map::new()),
                disabled: false,
                unsupported: Some("LNFU MCP server entry must be a JSON object".into()),
            });
            continue;
        };
        let (config, reason) = normalize_row(row, &mut container_reason);
        declarations.push(Declaration {
            identity,
            disabled: config
                .get("enabled")
                .and_then(Value::as_bool)
                .is_some_and(|enabled| !enabled),
            config,
            unsupported: reason,
        });
    }
    Ok((declarations, container_reason))
}

/// Validates one row and reduces it to the literal fields this adapter
/// understands. Unknown or indirect fields produce a reason; nothing is
/// substituted with a placeholder value.
fn normalize_row(
    row: &Map<String, Value>,
    container_reason: &mut Option<String>,
) -> (Value, Option<String>) {
    let mut reason: Option<String> = None;
    let transport = match row.get("transport") {
        Some(Value::String(transport))
            if transport == "stdio" || transport == "streamable-http" =>
        {
            transport.as_str()
        }
        Some(_) => {
            note(
                &mut reason,
                "LNFU MCP 'transport' must be the literal string stdio or streamable-http",
            );
            return (Value::Object(row.clone()), reason);
        }
        None => {
            note(
                &mut reason,
                "LNFU MCP entries must declare a literal 'transport'",
            );
            return (Value::Object(row.clone()), reason);
        }
    };
    let remote = transport == "streamable-http";
    let allowed: &[&str] = if remote {
        &[
            "name",
            "transport",
            "url",
            "headers",
            "toolCallTimeoutMs",
            "enabled",
        ]
    } else {
        &[
            "name",
            "transport",
            "command",
            "args",
            "env",
            "cwd",
            "toolCallTimeoutMs",
            "enabled",
        ]
    };
    if row.keys().any(|key| !allowed.contains(&key.as_str())) {
        note(
            &mut reason,
            "LNFU MCP configuration has unsupported fields; the effective server cannot be derived from literal text",
        );
    }
    match row.get("name") {
        Some(Value::String(name)) if valid_server_name(name) => {}
        Some(Value::String(_)) => note(
            &mut reason,
            "LNFU MCP server name must contain 1 to 32 ASCII letters, numbers, underscores or hyphens",
        ),
        Some(_) => note(&mut reason, "LNFU MCP server name must be a string"),
        None => note(
            &mut reason,
            "LNFU MCP server entry must declare a literal 'name'",
        ),
    }
    if let Some(value) = row.get("enabled") {
        if !value.is_boolean() {
            note(&mut reason, "LNFU MCP 'enabled' must be a literal boolean");
        }
    }
    match row.get("toolCallTimeoutMs") {
        Some(value)
            if !value
                .as_u64()
                .is_some_and(|timeout| (1..=MAX_EXTERNAL_MCP_TIMEOUT_MS).contains(&timeout)) =>
        {
            note(
                &mut reason,
                "LNFU toolCallTimeoutMs must be a positive bounded integer",
            );
        }
        _ => {}
    }
    if remote {
        match row.get("url") {
            Some(value) if bounded_string(value).is_some() => {}
            Some(_) => note(
                &mut reason,
                "LNFU remote MCP 'url' must be a bounded literal string",
            ),
            None => note(
                &mut reason,
                "LNFU remote MCP entries must declare a literal 'url'",
            ),
        }
        validate_string_map(row.get("headers"), "headers", &mut reason);
    } else {
        match row.get("command") {
            Some(value)
                if bounded_string(value).is_some_and(|command| !command.trim().is_empty()) => {}
            Some(_) => note(
                &mut reason,
                "LNFU MCP 'command' must be a non-empty literal string",
            ),
            None => note(
                &mut reason,
                "LNFU stdio MCP entries must declare a literal 'command'",
            ),
        }
        match row.get("args") {
            None => {}
            Some(Value::Array(args)) if args.len() <= 256 => {
                if args.iter().any(|arg| bounded_string(arg).is_none()) {
                    note(
                        &mut reason,
                        "LNFU MCP 'args' must be an array of at most 256 literal strings",
                    );
                }
            }
            Some(_) => note(
                &mut reason,
                "LNFU MCP 'args' must be an array of at most 256 literal strings",
            ),
        }
        validate_string_map(row.get("env"), "env", &mut reason);
        match row.get("cwd") {
            Some(value) if bounded_string(value).is_none() => {
                note(&mut reason, "LNFU MCP 'cwd' must be a literal string");
            }
            _ => {}
        }
    }
    // The row index keys each declaration identity, so two rows that share the
    // same literal name still get distinct ids; duplicates are reported apart.
    if container_reason.is_some() {
        note(
            &mut reason,
            "LNFU MCP document container is not a plain literal declaration list, so row semantics are not trusted",
        );
    }
    (Value::Object(row.clone()), reason)
}

fn note(reason: &mut Option<String>, message: &str) {
    reason.get_or_insert_with(|| message.to_string());
}

fn validate_string_map(value: Option<&Value>, field: &str, reason: &mut Option<String>) {
    let Some(value) = value else {
        return;
    };
    let Some(map) = value.as_object().filter(|map| map.len() <= 128) else {
        reason.get_or_insert(format!(
            "LNFU MCP '{field}' must be an object with at most 128 entries"
        ));
        return;
    };
    for (key, value) in map {
        if key.is_empty() || key.len() > 128 || key.chars().any(char::is_control) {
            reason.get_or_insert(format!("LNFU MCP '{field}' name is invalid"));
            return;
        }
        if bounded_string(value).is_none() {
            reason.get_or_insert(format!(
                "LNFU MCP '{field}' values must be bounded literal strings"
            ));
            return;
        }
        if value
            .as_str()
            .is_some_and(|text| text.contains("{env:") || text.contains("${"))
        {
            reason.get_or_insert(format!(
                "LNFU MCP '{field}' values must not rely on indirection this adapter cannot resolve statically"
            ));
            return;
        }
    }
}

fn bounded_string(value: &Value) -> Option<&str> {
    value
        .as_str()
        .filter(|text| text.len() <= 64 * 1024 && !text.contains('\0'))
}

fn valid_server_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 32
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
}

fn materialize_server(
    input: &ExternalMcpDiscoveryInput,
    source: &SourceKey,
    declaration: Declaration,
) -> Result<(ExternalMcpServerDefinition, PreparedExternalMcpTransport), ExternalSourceProviderError>
{
    if declaration.config.to_string().len() > 64 * 1024 {
        return Err(error(
            "config_invalid",
            "LNFU MCP declaration exceeds the 64 KiB runtime limit",
            false,
        ));
    }
    let mut reason = declaration.unsupported;
    let empty = Map::new();
    let config = declaration.config.as_object().unwrap_or(&empty);
    let remote = config.get("transport").and_then(Value::as_str) == Some("streamable-http");
    let raw_name = config
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let name = if valid_server_name(raw_name) {
        raw_name.to_string()
    } else {
        format!("invalid-{}", digest(declaration.identity.as_bytes()))
    };
    let id = SourceQualifiedMcpServerId::new(
        source.clone(),
        format!("mcp-{}", digest(declaration.identity.as_bytes())),
    )
    .expect("hashed id");
    let timeout = config
        .get("toolCallTimeoutMs")
        .and_then(Value::as_u64)
        .filter(|timeout| (1..=MAX_EXTERNAL_MCP_TIMEOUT_MS).contains(timeout))
        .unwrap_or(DEFAULT_CALL_TIMEOUT);
    let mut definition = ExternalMcpServerDefinition {
        id,
        provenance: vec![source.clone()],
        name,
        transport: if remote {
            ExternalMcpTransportKind::StreamableHttp
        } else {
            ExternalMcpTransportKind::LocalStdio
        },
        command_preview: None,
        argument_count: 0,
        working_directory: None,
        environment_keys: vec![],
        environment_reference_names: vec![],
        remote_url_preview: None,
        header_names: vec![],
        timeouts: ExternalMcpTimeouts {
            execution_ms: Some(timeout),
            ..Default::default()
        },
        source_enabled: !declaration.disabled,
        behavior_version: String::new(),
        static_status: ExternalMcpStaticStatus::Ready,
    };
    let transport = if remote {
        let raw_url = config
            .get("url")
            .and_then(bounded_string)
            .unwrap_or_default()
            .to_string();
        let mut headers = literal_map(config.get("headers"));
        let mut keys = BTreeSet::new();
        if headers
            .keys()
            .any(|key| !keys.insert(key.to_ascii_lowercase()))
        {
            reason.get_or_insert("LNFU MCP header names must be unique ignoring case".into());
            headers.clear();
        }
        // The preview is derived without a URL parser so the adapter stays
        // runtime-free and dependency-light: only the scheme and authority are
        // disclosed, and user information or a fragment is rejected outright.
        definition.remote_url_preview = Some(match https_authority_preview(&raw_url) {
            Some(preview) => preview,
            None => {
                reason.get_or_insert(
                    "LNFU remote MCP requires a valid literal HTTPS endpoint".into(),
                );
                "https://unsupported.invalid/".into()
            }
        });
        definition.header_names = headers.keys().cloned().collect();
        PreparedExternalMcpTransport::Remote {
            url: raw_url,
            headers: headers
                .into_iter()
                .map(|(key, value)| (key, SecretValue::new(value)))
                .collect(),
            oauth_enabled: false,
        }
    } else {
        let command = config
            .get("command")
            .and_then(bounded_string)
            .unwrap_or_default()
            .to_string();
        let args = match config.get("args") {
            Some(Value::Array(args)) if args.len() <= 256 => args
                .iter()
                .filter_map(|value| bounded_string(value).map(str::to_string))
                .collect::<Vec<_>>(),
            _ => vec![],
        };
        let environment = literal_map(config.get("env"));
        // LNFU resolves a relative cwd against the launch directory, never
        // against the declaration file. A selected workspace supplies it.
        let working_directory = match config.get("cwd").and_then(bounded_string) {
            None => input.context.workspace_root.clone(),
            Some(cwd) => {
                let path = PathBuf::from(cwd);
                if path.is_absolute() {
                    Some(path)
                } else {
                    match input.context.workspace_root.as_ref() {
                        Some(root) => Some(root.join(path)),
                        None => {
                            reason.get_or_insert(
                                "Relative LNFU MCP cwd requires a selected launch workspace".into(),
                            );
                            None
                        }
                    }
                }
            }
        };
        definition.command_preview = Some(redacted_executable_preview(&command));
        definition.argument_count = args.len();
        definition.working_directory = working_directory
            .as_ref()
            .map(|_| "<launch workspace or configured directory>".into());
        definition.environment_keys = environment.keys().cloned().collect();
        PreparedExternalMcpTransport::Local {
            command,
            args,
            environment: environment
                .into_iter()
                .map(|(key, value)| (key, SecretValue::new(value)))
                .collect(),
            working_directory,
        }
    };
    definition.static_status = if let Some(reason) = reason {
        ExternalMcpStaticStatus::Unsupported { reason }
    } else if declaration.disabled {
        ExternalMcpStaticStatus::DisabledBySource
    } else {
        ExternalMcpStaticStatus::Ready
    };
    let encoded = declaration.config.to_string();
    let cwd = match &transport {
        PreparedExternalMcpTransport::Local {
            working_directory, ..
        } => working_directory
            .as_ref()
            .map(|path| path.to_string_lossy().into_owned())
            .unwrap_or_default(),
        PreparedExternalMcpTransport::Remote { .. } => String::new(),
    };
    let status = serde_json::to_string(&definition.static_status).unwrap_or_default();
    definition.behavior_version = input.revision_key.opaque_revision(
        "lnfu.mcp.behavior.v1",
        [encoded.as_bytes(), cwd.as_bytes(), status.as_bytes()],
    );
    Ok((definition, transport))
}

/// Collects only literal string entries. Invalid shapes are already reported by
/// row normalization, so this stays a plain projection.
fn literal_map(value: Option<&Value>) -> BTreeMap<String, String> {
    let Some(map) = value.and_then(Value::as_object) else {
        return BTreeMap::new();
    };
    map.iter()
        .filter_map(|(key, value)| {
            bounded_string(value).map(|value| (key.clone(), value.to_string()))
        })
        .collect()
}

/// Accepts only a literal `https://<authority>[/...]` endpoint and returns a
/// preview that keeps the scheme and authority while dropping any path, query,
/// fragment, or user information. Returns `None` for anything else so callers
/// fail closed instead of guessing at an endpoint.
fn https_authority_preview(url: &str) -> Option<String> {
    let remainder = url.strip_prefix("https://")?;
    if remainder.is_empty() || url.chars().any(char::is_control) {
        return None;
    }
    let authority = remainder
        .split(['/', '?', '#'])
        .next()
        .filter(|authority| !authority.is_empty())?;
    if authority.contains('@') || !authority.contains('.') {
        return None;
    }
    if authority.len() > 255
        || !authority.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b':' | b'[' | b']' | b'_')
        })
    {
        return None;
    }
    Some(format!("https://{authority}/"))
}

fn digest(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))[..24].to_string()
}

fn source_key(path: &Path) -> SourceKey {
    SourceKey::new(
        PROVIDER,
        format!("lnfu_mcp-{}", digest(path.to_string_lossy().as_bytes())),
    )
    .expect("hashed id")
}

fn error(suffix: &str, message: &str, transient: bool) -> ExternalSourceProviderError {
    ExternalSourceProviderError::new(format!("lnfu.mcp.{suffix}"), message, transient)
}

fn diagnostic(source: &SourceKey, suffix: &str, message: &str) -> ExternalSourceDiagnostic {
    ExternalSourceDiagnostic::warning(format!("lnfu.mcp.{suffix}"), message, Some(source.clone()))
        .with_asset_kind(ExternalSourceAssetKind::Mcp)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(_home: &Path, workspace: Option<PathBuf>) -> ExternalMcpDiscoveryInput {
        ExternalMcpDiscoveryInput {
            context: ExternalSourceContext {
                workspace_root: workspace,
                execution_domain_id: ExecutionDomainId::new("fixture").unwrap(),
            },
            suppressed_sources: BTreeSet::new(),
            revision_key: ExternalMcpRevisionKey::new([7u8; 32]),
        }
    }

    fn write(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    #[test]
    fn discovers_literal_stdio_and_remote_declarations() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        write(
            &home.join("mcp.json"),
            r#"{"servers":[
                {"name":"local_echo","transport":"stdio","command":"node","args":["s.js"],"env":{"TOKEN":"SECRET"}},
                {"name":"remote_feed","transport":"streamable-http","url":"https://example.test/mcp","headers":{"Authorization":"Bearer SECRET"}}
            ]}"#,
        );
        let provider = LnfuMcpProvider::new(LnfuMcpProviderOptions {
            lnfu_home: home.clone(),
        });
        let snapshot = provider.discover(&input(&home, None)).unwrap();
        assert_eq!(snapshot.provider.ecosystem_id.as_str(), "lnfu");
        assert_eq!(snapshot.sources.len(), 1);
        assert_eq!(snapshot.sources[0].source_kind, "lnfu_mcp_config");
        assert_eq!(snapshot.servers.len(), 2);
        assert!(snapshot
            .servers
            .iter()
            .all(|server| server.static_status == ExternalMcpStaticStatus::Ready));
        let local = snapshot
            .servers
            .iter()
            .find(|server| server.name == "local_echo")
            .unwrap();
        assert_eq!(local.transport, ExternalMcpTransportKind::LocalStdio);
        assert_eq!(local.argument_count, 1);
        assert_eq!(local.environment_keys, vec!["TOKEN".to_string()]);
        let remote = snapshot
            .servers
            .iter()
            .find(|server| server.name == "remote_feed")
            .unwrap();
        assert_eq!(remote.transport, ExternalMcpTransportKind::StreamableHttp);
        assert_eq!(
            remote.remote_url_preview.as_deref(),
            Some("https://example.test/")
        );
        let encoded = serde_json::to_string(&snapshot).unwrap();
        assert!(!encoded.contains("SECRET"));
    }

    #[test]
    fn indirect_or_unknown_declarations_are_unsupported_without_placeholders() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        write(
            &home.join("mcp.json"),
            r#"{"servers":[
                {"name":"indirect","transport":"stdio","command":"node","env":{"TOKEN":"${LNFU_TOKEN}"}},
                {"name":"extra","transport":"stdio","command":"node","reconnect":{"enabled":true}}
            ]}"#,
        );
        let snapshot = LnfuMcpProvider::new(LnfuMcpProviderOptions {
            lnfu_home: home.clone(),
        })
        .discover(&input(&home, None))
        .unwrap();
        assert_eq!(snapshot.servers.len(), 2);
        assert!(snapshot.servers.iter().all(|server| matches!(
            server.static_status,
            ExternalMcpStaticStatus::Unsupported { .. }
        )));
        assert_eq!(snapshot.sources[0].health, ExternalSourceHealth::Available);
    }

    #[test]
    fn prepare_server_returns_the_literal_transport_and_rejects_stale_versions() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        write(
            &home.join("mcp.json"),
            r#"{"servers":[{"name":"local_echo","transport":"stdio","command":"node","args":["s.js"],"cwd":"sub"}]}"#,
        );
        let workspace = temp.path().join("workspace");
        std::fs::create_dir_all(&workspace).unwrap();
        let provider = LnfuMcpProvider::new(LnfuMcpProviderOptions {
            lnfu_home: home.clone(),
        });
        let input = input(&home, Some(workspace.clone()));
        let snapshot = provider.discover(&input).unwrap();
        let server = &snapshot.servers[0];
        let prepared = provider
            .prepare_server(&input, &server.id, &server.behavior_version)
            .unwrap();
        match prepared.transport {
            PreparedExternalMcpTransport::Local {
                command,
                args,
                working_directory,
                environment,
            } => {
                assert_eq!(command, "node");
                assert_eq!(args, vec!["s.js".to_string()]);
                assert!(environment.is_empty());
                assert_eq!(working_directory, Some(workspace.join("sub")));
            }
            PreparedExternalMcpTransport::Remote { .. } => panic!("expected local transport"),
        }
        assert!(provider
            .prepare_server(&input, &server.id, "hmac-sha256:stale")
            .is_err());
    }

    #[test]
    fn import_projection_is_explicitly_unsupported() {
        let provider = LnfuMcpProvider::default();
        let temp = tempfile::tempdir().unwrap();
        let input = input(temp.path(), None);
        let snapshot = provider.discover(&input).unwrap();
        let id =
            SourceQualifiedMcpServerId::new(SourceKey::new(PROVIDER, "missing").unwrap(), "mcp-0")
                .unwrap();
        let error = provider.prepare_import(&input, &id, "v").unwrap_err();
        assert!(error.code.starts_with("external_mcp.import_"));
        assert!(snapshot.servers.is_empty());
    }

    #[test]
    fn watch_roots_include_lnfu_home_and_the_selected_workspace() {
        let provider = LnfuMcpProvider::new(LnfuMcpProviderOptions {
            lnfu_home: PathBuf::from("C:/lnfu-home"),
        });
        let workspace = PathBuf::from("C:/workspace");
        let roots = provider.watch_roots(&ExternalSourceContext {
            workspace_root: Some(workspace.clone()),
            execution_domain_id: ExecutionDomainId::new("fixture").unwrap(),
        });
        assert_eq!(roots.len(), 2);
        assert_eq!(roots[0].path, PathBuf::from("C:/lnfu-home"));
        assert!(roots[0].recursive);
        assert_eq!(roots[1].path, workspace);
        assert!(!roots[1].recursive);
    }

    #[test]
    fn declarations_outside_the_source_root_are_rejected() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        std::fs::create_dir_all(&home).unwrap();
        let provider = LnfuMcpProvider::new(LnfuMcpProviderOptions {
            lnfu_home: home.clone(),
        });
        assert!(provider
            .discover(&input(&home, None))
            .unwrap()
            .servers
            .is_empty());
        write(&home.join("mcp.json"), "not json");
        let error = provider.discover(&input(&home, None)).unwrap_err();
        assert_eq!(error.code, "lnfu.mcp.config_invalid");
    }
}
