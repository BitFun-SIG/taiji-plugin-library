//! Seam: `storage` - Service Definition `StoragePort`.
//!
//! Contract reference: CONTRACT.md section 2.1 row `storage`. Provider for the
//! first phase is a JSON writer plus snapshot reader; the consumer is the set of
//! domain components.
//!
//! All paths handed to this seam are resolved by the config seam
//! (CONTRACT.md section 0.3 placeholders); the storage seam never composes an
//! absolute path literal of its own.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::ContractError;

/// Error code: storage write failed.
pub const E_STORAGE_001: &str = "E-STORAGE-001";
/// Error code: storage read failed.
pub const E_STORAGE_002: &str = "E-STORAGE-002";
/// Error code: stored payload is not valid JSON.
pub const E_STORAGE_003: &str = "E-STORAGE-003";

/// Domain tag used on storage errors.
pub const DOMAIN_STORAGE: &str = "STORAGE";

/// Service Definition for the storage seam.
pub trait StoragePort: Send + Sync {
    /// Provider name.
    fn provider_name(&self) -> &str;
    /// Write a JSON document at the given path, creating parent directories.
    fn write_json(&self, path: &Path, value: &serde_json::Value) -> Result<usize, ContractError>;
    /// Read a JSON snapshot from the given path.
    fn read_json(&self, path: &Path) -> Result<serde_json::Value, ContractError>;
    /// Whether a stored document exists at the given path.
    fn exists(&self, path: &Path) -> bool;
}

/// First-phase provider: JSON document writer and snapshot reader.
#[derive(Debug, Default)]
pub struct JsonFileStorage;

impl JsonFileStorage {
    /// Build the provider.
    pub fn new() -> Self {
        Self
    }
}

impl StoragePort for JsonFileStorage {
    fn provider_name(&self) -> &str {
        "json_file"
    }

    fn write_json(&self, path: &Path, value: &serde_json::Value) -> Result<usize, ContractError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|err| {
                ContractError::here(
                    E_STORAGE_001,
                    DOMAIN_STORAGE,
                    format!(
                        "cannot create parent directory for {}: {err}",
                        path.display()
                    ),
                )
            })?;
        }
        let mut body = serde_json::to_string_pretty(value).map_err(|err| {
            ContractError::here(
                E_STORAGE_003,
                DOMAIN_STORAGE,
                format!("cannot serialize document: {err}"),
            )
        })?;
        body.push('\n');
        std::fs::write(path, body.as_bytes()).map_err(|err| {
            ContractError::here(
                E_STORAGE_001,
                DOMAIN_STORAGE,
                format!("cannot write {}: {err}", path.display()),
            )
        })?;
        Ok(body.len())
    }

    fn read_json(&self, path: &Path) -> Result<serde_json::Value, ContractError> {
        let raw = std::fs::read_to_string(path).map_err(|err| {
            ContractError::here(
                E_STORAGE_002,
                DOMAIN_STORAGE,
                format!("cannot read {}: {err}", path.display()),
            )
        })?;
        serde_json::from_str(&raw).map_err(|err| {
            ContractError::here(
                E_STORAGE_003,
                DOMAIN_STORAGE,
                format!("invalid JSON in {}: {err}", path.display()),
            )
        })
    }

    fn exists(&self, path: &Path) -> bool {
        path.is_file()
    }
}

/// A stored document paired with its resolved path.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StorageSnapshot {
    /// Identifier of the document owner.
    pub owner: String,
    /// Resolved absolute path of the stored document.
    pub path: PathBuf,
    /// Stored document body.
    pub body: serde_json::Value,
}
