//! Seam: `system_prompt` - Service Definition `SystemPromptPort`.
//!
//! Contract reference: CONTRACT.md section 2.1 row `system_prompt`. Provider for
//! the first phase is `StaticPromptProvider`; the consumer is request assembly.

use serde::{Deserialize, Serialize};

use crate::error::ContractError;

/// Error code: prompt provider not registered.
pub const E_SEAM_001: &str = "E-SEAM-001";

/// Domain tag used on system prompt errors.
pub const DOMAIN_PROMPT: &str = "PROMPT";

/// One assembled prompt section, in assembly order.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PromptSection {
    /// Section name, for example `identity` or `policy`.
    pub name: String,
    /// Section body text.
    pub body: String,
}

/// Service Definition for the system prompt seam.
pub trait SystemPromptPort: Send + Sync {
    /// Provider name.
    fn provider_name(&self) -> &str;
    /// Ordered prompt sections for the given assembly key.
    fn sections(&self, key: &str) -> Result<Vec<PromptSection>, ContractError>;
}

/// First-phase provider: a fixed set of sections held in order.
#[derive(Debug, Default)]
pub struct StaticPromptProvider {
    sections: Vec<PromptSection>,
}

impl StaticPromptProvider {
    /// Build an empty static provider.
    pub fn new() -> Self {
        Self {
            sections: Vec::new(),
        }
    }

    /// Build a static provider from an ordered section list.
    pub fn from_sections(sections: Vec<PromptSection>) -> Self {
        Self { sections }
    }

    /// Append a section, keeping assembly order.
    pub fn push(&mut self, section: PromptSection) {
        self.sections.push(section);
    }
}

impl SystemPromptPort for StaticPromptProvider {
    fn provider_name(&self) -> &str {
        "static"
    }

    fn sections(&self, key: &str) -> Result<Vec<PromptSection>, ContractError> {
        if key.is_empty() {
            return Err(ContractError::here(
                E_SEAM_001,
                DOMAIN_PROMPT,
                "system prompt assembly key must be present",
            ));
        }
        Ok(self.sections.clone())
    }
}
