//! Seam: `subagent` - Service Definition `SubagentPort`.
//!
//! Contract reference: CONTRACT.md section 2.1 row `subagent`. Provider for the
//! first phase is `NullSubagentRuntime`, which fails loud with
//! `Availability::Unavailable { reason }` instead of silently succeeding; the
//! consumer is the dispatch face. The reserved-but-unimplemented pattern follows
//! RECON-20260912-dsh-rust-refs.md section 2.5 M4.

use serde::{Deserialize, Serialize};

use crate::availability::Availability;
use crate::error::ContractError;

/// Error code: subagent runtime not registered.
pub const E_SEAM_001: &str = "E-SEAM-001";

/// Domain tag used on subagent errors.
pub const DOMAIN_SUBAGENT: &str = "SUBAGENT";

/// Reason string carried by the null runtime.
pub const NULL_SUBAGENT_REASON: &str =
    "subagent runtime is not registered in this delivery profile";

/// Specification of a child run handed to the subagent runtime.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SubagentSpec {
    /// Identity of the subagent to run.
    pub agent_id: String,
    /// Instruction text handed to the subagent.
    pub instruction: String,
}

/// Terminal outcome of a subagent run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SubagentOutcome {
    /// Identity of the subagent that ran.
    pub agent_id: String,
    /// Terminal state token, for example `completed` or `failed`.
    pub state: String,
    /// Summary text produced by the subagent.
    pub summary: String,
}

/// Service Definition for the subagent seam.
pub trait SubagentPort: Send + Sync {
    /// Availability of this runtime; unregistered runtimes report `Unavailable`.
    fn availability(&self) -> Availability;
    /// Run one subagent; an unavailable runtime fails with `E-SEAM-001`.
    fn run(&self, spec: &SubagentSpec) -> Result<SubagentOutcome, ContractError>;
}

/// First-phase provider: an explicitly unavailable subagent runtime.
#[derive(Debug, Default)]
pub struct NullSubagentRuntime;

impl NullSubagentRuntime {
    /// Build the null runtime.
    pub fn new() -> Self {
        Self
    }
}

impl SubagentPort for NullSubagentRuntime {
    fn availability(&self) -> Availability {
        Availability::unavailable(NULL_SUBAGENT_REASON)
    }

    fn run(&self, spec: &SubagentSpec) -> Result<SubagentOutcome, ContractError> {
        Err(ContractError::here(
            E_SEAM_001,
            DOMAIN_SUBAGENT,
            format!(
                "seam subagent unavailable for agent {}: {}",
                spec.agent_id, NULL_SUBAGENT_REASON
            ),
        ))
    }
}
