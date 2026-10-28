//! Seam: `config` - Service Definition `ConfigPort`.
//!
//! Contract reference: CONTRACT.md section 2.1 row `config` and section 0.3
//! landing root coordinates. The provider for the first phase resolves every
//! placeholder from environment variables and startup arguments; the consumer is
//! the assembly layer.
//!
//! Resolution priority follows CONTRACT.md section 0.3 exactly. No absolute path
//! literal and no fixed port value is used as a sole source anywhere in this
//! module: the leftmost source that yields a value wins, and the terminal source
//! is dynamic discovery.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::ContractError;

/// Error code: required configuration key missing.
pub const E_CONFIG_001: &str = "E-CONFIG-001";
/// Error code: startup argument malformed.
pub const E_CONFIG_002: &str = "E-CONFIG-002";

/// Domain tag used on configuration errors.
pub const DOMAIN_CONFIG: &str = "CONFIG";

/// Environment variable name for the kernel root.
pub const ENV_KERNEL_ROOT: &str = "LNFU_KERNEL_ROOT";
/// Environment variable name for the universe root.
pub const ENV_UNIVERSE_ROOT: &str = "LNFU_UNIVERSE_ROOT";
/// Environment variable name for the panel port.
pub const ENV_PANEL_PORT: &str = "LNFU_PANEL_PORT";
/// Environment variable name for the graph page byte limit.
pub const ENV_GRAPH_PAGE_BYTES: &str = "LNFU_GRAPH_PAGE_BYTES";
/// Environment variable name for the panel bind address.
pub const ENV_PANEL_BIND: &str = "LNFU_PANEL_BIND";

/// Startup argument name for the universe root.
pub const ARG_UNIVERSE_ROOT: &str = "--universe-root";
/// Startup argument name for the panel port.
pub const ARG_PANEL_PORT: &str = "--panel-port";

/// Directory name of worktrees below the universe root (CONTRACT.md section 0.3).
pub const WORKTREE_DIR_NAME: &str = "worktrees";
/// Audit file name at the universe root (CONTRACT.md section 0.3).
pub const AUDIT_FILE_NAME: &str = "audit.log";
/// Result file name template inside a worktree (CONTRACT.md section 0.3). The
/// `{role_id}` coordinate is resolved by the caller through [`ConfigPort::result_file`].
pub const RESULT_FILE_NAME: &str = "result-{role_id}.json";
/// Default graph page byte limit when the environment variable is absent.
pub const DEFAULT_GRAPH_PAGE_BYTES: u64 = 262_144;

/// Resolve the `__RESULT_FILE__` template (CONTRACT.md section 0.3 item 5) for a
/// role coordinate. Single resolution point of the result file name.
pub fn result_file_name(role_id: &str) -> String {
    format!("result-{role_id}.json")
}

/// Sources consulted while resolving configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfigSource {
    /// Value came from an environment variable.
    Environment,
    /// Value came from a startup argument.
    Argument,
    /// Value came from dynamic discovery.
    Discovered,
}

/// A resolved coordinate together with the source that supplied it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Resolved<T> {
    /// Resolved value.
    pub value: T,
    /// Source that supplied the value.
    pub source: ConfigSource,
}

/// Parsed startup arguments.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct StartupArgs {
    /// Resolved universe root, when supplied through arguments.
    pub universe_root: Option<PathBuf>,
    /// Resolved panel port, when supplied through arguments.
    pub panel_port: Option<u16>,
}

impl StartupArgs {
    /// Parse a startup argument list.
    ///
    /// Both `--name value` and `--name=value` forms are accepted. A malformed
    /// pair fails with `E-CONFIG-002`.
    pub fn parse<I, S>(args: I) -> Result<Self, ContractError>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let collected: Vec<String> = args.into_iter().map(|a| a.as_ref().to_string()).collect();
        let mut parsed = StartupArgs::default();
        let mut index = 0usize;
        while index < collected.len() {
            let token = collected[index].clone();
            let (name, inline) = match token.split_once('=') {
                Some((name, value)) => (name.to_string(), Some(value.to_string())),
                None => (token, None),
            };
            let take_value = |index: &mut usize| -> Result<String, ContractError> {
                if let Some(value) = inline.clone() {
                    return Ok(value);
                }
                *index += 1;
                collected.get(*index).cloned().ok_or_else(|| {
                    ContractError::new(
                        E_CONFIG_002,
                        DOMAIN_CONFIG,
                        format!("argument {name} requires a value"),
                        "contracts/src/config.rs",
                    )
                })
            };
            match name.as_str() {
                ARG_UNIVERSE_ROOT => {
                    let value = take_value(&mut index)?;
                    parsed.universe_root = Some(PathBuf::from(value));
                }
                ARG_PANEL_PORT => {
                    let value = take_value(&mut index)?;
                    let port: u16 = value.parse().map_err(|err| {
                        ContractError::new(
                            E_CONFIG_002,
                            DOMAIN_CONFIG,
                            format!(
                                "argument {ARG_PANEL_PORT} value is not a port: {value} ({err})"
                            ),
                            "contracts/src/config.rs",
                        )
                    })?;
                    parsed.panel_port = Some(port);
                }
                _ => {}
            }
            index += 1;
        }
        Ok(parsed)
    }
}

/// Service Definition for the configuration seam.
pub trait ConfigPort: Send + Sync {
    /// Provider name.
    fn provider_name(&self) -> &str;
    /// Resolve the kernel root.
    fn kernel_root(&self) -> Result<PathBuf, ContractError>;
    /// Resolve the universe root.
    fn universe_root(&self) -> Result<PathBuf, ContractError>;
    /// Resolve the worktree directory below the universe root.
    fn worktree_dir(&self) -> Result<PathBuf, ContractError>;
    /// Resolve the audit file path at the universe root.
    fn audit_file(&self) -> Result<PathBuf, ContractError>;
    /// Resolve the result file name inside a worktree for a given role.
    ///
    /// The `__RESULT_FILE__` coordinate is a template carrying the role id
    /// (CONTRACT.md section 0.3 item 5), so the caller supplies the role whose
    /// product the file will carry.
    fn result_file(&self, role_id: &str) -> String;
    /// Resolve the panel port.
    fn panel_port(&self) -> Result<Resolved<u16>, ContractError>;
    /// Resolve the panel origin assembled from the bind address and port.
    fn panel_origin(&self) -> Result<String, ContractError>;
    /// Resolve the graph page byte limit.
    fn graph_page_bytes(&self) -> u64;
    /// Resolve the loopback bind address for the panel.
    fn panel_bind(&self) -> String;
}

/// First-phase provider: environment variables plus startup argument parser.
#[derive(Debug, Clone)]
pub struct EnvConfigProvider {
    args: StartupArgs,
    env: BTreeMap<String, String>,
    discovered_kernel_root: Option<PathBuf>,
}

impl Default for EnvConfigProvider {
    fn default() -> Self {
        Self::from_process_env()
    }
}

impl EnvConfigProvider {
    /// Build a provider from explicit inputs; used by tests and by callers that
    /// already parsed their own argument list.
    pub fn new(args: StartupArgs, env: BTreeMap<String, String>) -> Self {
        Self {
            args,
            env,
            discovered_kernel_root: None,
        }
    }

    /// Build a provider reading the ambient environment and process arguments.
    pub fn from_process_env() -> Self {
        let env: BTreeMap<String, String> = std::env::vars().collect();
        let args = StartupArgs::parse(std::env::args().skip(1)).unwrap_or_default();
        Self::new(args, env)
    }

    /// Override the dynamically discovered kernel root.
    pub fn with_discovered_kernel_root(mut self, root: impl Into<PathBuf>) -> Self {
        self.discovered_kernel_root = Some(root.into());
        self
    }

    fn env_value(&self, key: &str) -> Option<String> {
        self.env
            .get(key)
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
    }

    /// Root directory of the running executable, used as the self-locating
    /// fallback required by CONTRACT.md section 0.3.
    fn executable_root() -> Option<PathBuf> {
        let exe = std::env::current_exe().ok()?;
        exe.parent().map(Path::to_path_buf)
    }
}

impl ConfigPort for EnvConfigProvider {
    fn provider_name(&self) -> &str {
        "env_and_args"
    }

    fn kernel_root(&self) -> Result<PathBuf, ContractError> {
        if let Some(value) = self.env_value(ENV_KERNEL_ROOT) {
            return Ok(PathBuf::from(value));
        }
        // Fallback: self-location resolved by the caller, then the executable
        // directory. Neither branch uses an absolute path literal.
        if let Some(root) = &self.discovered_kernel_root {
            return Ok(root.clone());
        }
        Self::executable_root().ok_or_else(|| {
            ContractError::here(
                E_CONFIG_001,
                DOMAIN_CONFIG,
                format!(
                    "kernel root unresolved: set {ENV_KERNEL_ROOT} or provide a self-located root"
                ),
            )
        })
    }

    fn universe_root(&self) -> Result<PathBuf, ContractError> {
        if let Some(value) = self.env_value(ENV_UNIVERSE_ROOT) {
            return Ok(PathBuf::from(value));
        }
        if let Some(root) = &self.args.universe_root {
            return Ok(root.clone());
        }
        Err(ContractError::here(
            E_CONFIG_001,
            DOMAIN_CONFIG,
            format!(
                "universe root unresolved: set {ENV_UNIVERSE_ROOT} or pass {ARG_UNIVERSE_ROOT}"
            ),
        ))
    }

    fn worktree_dir(&self) -> Result<PathBuf, ContractError> {
        Ok(self.universe_root()?.join(WORKTREE_DIR_NAME))
    }

    fn audit_file(&self) -> Result<PathBuf, ContractError> {
        Ok(self.universe_root()?.join(AUDIT_FILE_NAME))
    }

    fn result_file(&self, role_id: &str) -> String {
        result_file_name(role_id)
    }

    fn panel_port(&self) -> Result<Resolved<u16>, ContractError> {
        if let Some(value) = self.env_value(ENV_PANEL_PORT) {
            let port: u16 = value.parse().map_err(|err| {
                ContractError::here(
                    E_CONFIG_002,
                    DOMAIN_CONFIG,
                    format!("{ENV_PANEL_PORT} is not a port: {value} ({err})"),
                )
            })?;
            return Ok(Resolved {
                value: port,
                source: ConfigSource::Environment,
            });
        }
        if let Some(port) = self.args.panel_port {
            return Ok(Resolved {
                value: port,
                source: ConfigSource::Argument,
            });
        }
        // Terminal source: dynamic discovery by the operating system.
        let listener = std::net::TcpListener::bind((self.panel_bind(), 0)).map_err(|err| {
            ContractError::here(
                E_CONFIG_001,
                DOMAIN_CONFIG,
                format!("cannot discover a free panel port: {err}"),
            )
        })?;
        let port = listener.local_addr().map_err(|err| {
            ContractError::here(
                E_CONFIG_001,
                DOMAIN_CONFIG,
                format!("cannot read the discovered port: {err}"),
            )
        })?;
        Ok(Resolved {
            value: port.port(),
            source: ConfigSource::Discovered,
        })
    }

    fn panel_origin(&self) -> Result<String, ContractError> {
        let port = self.panel_port()?.value;
        Ok(format!("http://{}:{}", self.panel_bind(), port))
    }

    fn graph_page_bytes(&self) -> u64 {
        self.env_value(ENV_GRAPH_PAGE_BYTES)
            .and_then(|value| value.parse().ok())
            .unwrap_or(DEFAULT_GRAPH_PAGE_BYTES)
    }

    fn panel_bind(&self) -> String {
        self.env_value(ENV_PANEL_BIND)
            .unwrap_or_else(|| "127.0.0.1".to_string())
    }
}
