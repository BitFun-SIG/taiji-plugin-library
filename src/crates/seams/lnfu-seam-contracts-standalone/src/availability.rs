//! Capability availability discipline.
//!
//! Contract reference: CONTRACT.md section 2.1 defines `Availability`, and the
//! recon report RECON-20260912-dsh-rust-refs.md section 2.5 M3 fixes the rule:
//! an unregistered capability fails loud with an explicit reason instead of
//! being skipped silently.

use serde::{Deserialize, Serialize};

/// Availability of a capability seam or of a provider behind it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Availability {
    /// The capability is registered and usable.
    Available,
    /// The capability is intentionally absent; `reason` states why.
    Unavailable { reason: String },
}

impl Availability {
    /// Build an unavailable marker carrying the reason.
    pub fn unavailable(reason: impl Into<String>) -> Self {
        Self::Unavailable {
            reason: reason.into(),
        }
    }

    /// Build an available marker.
    pub fn available() -> Self {
        Self::Available
    }

    /// Whether the capability can be used.
    pub fn is_available(&self) -> bool {
        matches!(self, Self::Available)
    }

    /// Render the availability as a stable machine readable token.
    ///
    /// The token is used by the startup configuration dump, so an unavailable
    /// capability stays visible to the operator (CONTRACT.md section 2.5).
    pub fn as_token(&self) -> String {
        match self {
            Self::Available => "available".to_string(),
            Self::Unavailable { reason } => format!("unavailable: {reason}"),
        }
    }
}
