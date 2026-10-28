//! Seam: `web_panel` - Service Definition `WebPanelPort`.
//!
//! Contract reference: CONTRACT.md section 2.1 row `web_panel` and section 2.4
//! panel seam. Provider for the first phase is an empty page plus a same-origin
//! data endpoint; the consumer is the frontend view.
//!
//! Data source: the panel reads Git repository instance state through the
//! topology text supplied by its caller; there is no side data source
//! (CONTRACT.md section 2.4, test predicate 2).

use serde::{Deserialize, Serialize};

use crate::error::ContractError;

/// Error code: panel port not bound.
pub const E_PANEL_001: &str = "E-PANEL-001";
/// Error code: cross-origin request rejected.
pub const E_PANEL_002: &str = "E-PANEL-002";
/// Error code: topology generation failed.
pub const E_PANEL_003: &str = "E-PANEL-003";

/// Domain tag used on panel errors.
pub const DOMAIN_PANEL: &str = "PANEL";

/// Path of the topology data endpoint.
pub const TOPOLOGY_PATH: &str = "/api/topology/mermaid";
/// Content type returned by the topology endpoint.
pub const TOPOLOGY_CONTENT_TYPE: &str = "text/plain; charset=utf-8";
/// Root page path.
pub const ROOT_PATH: &str = "/";

/// One panel response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PanelResponse {
    /// HTTP status code.
    pub status: u16,
    /// Response content type.
    pub content_type: String,
    /// Response body.
    pub body: String,
}

/// Service Definition for the web panel seam.
pub trait WebPanelPort: Send + Sync {
    /// Provider name.
    fn provider_name(&self) -> &str;
    /// Handle one request against the panel origin.
    fn handle(&self, path: &str, origin: Option<&str>) -> Result<PanelResponse, ContractError>;
}

/// First-phase provider: empty page plus same-origin topology endpoint.
#[derive(Debug, Default)]
pub struct EmptyWebPanel {
    topology: String,
}

impl EmptyWebPanel {
    /// Build the panel with an empty topology document.
    pub fn new() -> Self {
        Self {
            topology: String::new(),
        }
    }

    /// Build the panel with a topology document; the first line must start with
    /// `graph`, as fixed by CONTRACT.md section 2.4.
    pub fn with_topology(topology: impl Into<String>) -> Result<Self, ContractError> {
        let topology = topology.into();
        let first_line = topology.lines().next().unwrap_or_default().trim();
        if !first_line.starts_with("graph") {
            return Err(ContractError::here(
                E_PANEL_003,
                DOMAIN_PANEL,
                "topology document must start with a graph declaration",
            ));
        }
        Ok(Self { topology })
    }

    /// The topology document held by this panel.
    pub fn topology(&self) -> &str {
        &self.topology
    }
}

impl WebPanelPort for EmptyWebPanel {
    fn provider_name(&self) -> &str {
        "empty_panel"
    }

    fn handle(&self, path: &str, origin: Option<&str>) -> Result<PanelResponse, ContractError> {
        if let Some(origin) = origin {
            if origin.is_empty() {
                return Err(ContractError::here(
                    E_PANEL_002,
                    DOMAIN_PANEL,
                    "cross-origin request rejected: empty origin",
                ));
            }
        }
        match path {
            ROOT_PATH => Ok(PanelResponse {
                status: 200,
                content_type: "text/html; charset=utf-8".to_string(),
                body: "<!doctype html><html><head><meta charset=\"utf-8\"><title>LNFU</title></head><body></body></html>".to_string(),
            }),
            TOPOLOGY_PATH => Ok(PanelResponse {
                status: 200,
                content_type: TOPOLOGY_CONTENT_TYPE.to_string(),
                body: self.topology.clone(),
            }),
            other => Ok(PanelResponse {
                status: 404,
                content_type: TOPOLOGY_CONTENT_TYPE.to_string(),
                body: format!("no panel route for {other}"),
            }),
        }
    }
}
