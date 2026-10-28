//! Seam descriptor and the first-phase seam identifier set.
//!
//! Contract reference: CONTRACT.md section 2.1 fixes the eight seam ids, their
//! service definition names, their first-phase minimal implementation and their
//! consumer. Each seam must carry all three roles: Service Definition (port),
//! Provider (implementation) and Consumer; one role alone is not a seam
//! (RECON-20260912-dsh-rust-refs.md section 2.4).

use serde::{Deserialize, Serialize};

use crate::availability::Availability;

/// Stable identifier of a capability seam.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SeamId {
    /// Tool registry seam.
    ToolRegistry,
    /// Model provider seam.
    ModelProvider,
    /// Subagent seam.
    Subagent,
    /// System prompt seam.
    SystemPrompt,
    /// Runtime event bus seam.
    EventBus,
    /// Storage seam.
    Storage,
    /// Configuration seam.
    Config,
    /// Web panel seam.
    WebPanel,
}

impl SeamId {
    /// All eight first-phase seams in contract order.
    pub const ALL: [SeamId; 8] = [
        SeamId::ToolRegistry,
        SeamId::ModelProvider,
        SeamId::Subagent,
        SeamId::SystemPrompt,
        SeamId::EventBus,
        SeamId::Storage,
        SeamId::Config,
        SeamId::WebPanel,
    ];

    /// Contract identifier string, identical to the seam id column of
    /// CONTRACT.md section 2.1.
    pub const fn as_str(&self) -> &'static str {
        match self {
            SeamId::ToolRegistry => "tool_registry",
            SeamId::ModelProvider => "model_provider",
            SeamId::Subagent => "subagent",
            SeamId::SystemPrompt => "system_prompt",
            SeamId::EventBus => "event_bus",
            SeamId::Storage => "storage",
            SeamId::Config => "config",
            SeamId::WebPanel => "web_panel",
        }
    }

    /// Service definition (port trait) name fixed by CONTRACT.md section 2.1.
    pub const fn definition(&self) -> &'static str {
        match self {
            SeamId::ToolRegistry => "ToolRegistryPort",
            SeamId::ModelProvider => "LlmProviderPort",
            SeamId::Subagent => "SubagentPort",
            SeamId::SystemPrompt => "SystemPromptPort",
            SeamId::EventBus => "RuntimeEventSink",
            SeamId::Storage => "StoragePort",
            SeamId::Config => "ConfigPort",
            SeamId::WebPanel => "WebPanelPort",
        }
    }

    /// Consumer role fixed by CONTRACT.md section 2.1.
    pub const fn consumer(&self) -> &'static str {
        match self {
            SeamId::ToolRegistry => "model_visible_tool_assembly",
            SeamId::ModelProvider => "LlmFactory",
            SeamId::Subagent => "dispatch_face",
            SeamId::SystemPrompt => "request_assembly",
            SeamId::EventBus => "FractalRouter",
            SeamId::Storage => "domain_components",
            SeamId::Config => "assembly_layer",
            SeamId::WebPanel => "frontend_view",
        }
    }
}

impl std::fmt::Display for SeamId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Descriptor exported by a registered seam.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SeamDescriptor {
    pub id: String,
    pub definition: String,
    pub provider: String,
    pub consumer: String,
    pub availability: Availability,
}

impl SeamDescriptor {
    /// Build a descriptor for a seam id from its provider name and availability.
    pub fn new(id: SeamId, provider: impl Into<String>, availability: Availability) -> Self {
        Self {
            id: id.as_str().to_string(),
            definition: id.definition().to_string(),
            provider: provider.into(),
            consumer: id.consumer().to_string(),
            availability,
        }
    }

    /// Whether all three seam roles are present.
    pub fn is_complete(&self) -> bool {
        !self.id.is_empty() && !self.definition.is_empty() && !self.provider.is_empty()
    }
}
