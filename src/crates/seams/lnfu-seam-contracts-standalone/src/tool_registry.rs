//! Seam: `tool_registry` - Service Definition `ToolRegistryPort`.
//!
//! Contract reference: CONTRACT.md section 2.1 row `tool_registry`. Provider for
//! the first phase is an ordered explicit registry (`IndexMap`, order preserved);
//! the consumer is model-visible tool assembly. Registration returns a disposer
//! and revocation follows the mechanism fixed by section 2.5.

use serde::{Deserialize, Serialize};

use crate::error::ContractError;
use crate::registry::{Disposer, SharedRegistry};

/// Error code: tool schema is not a JSON object.
pub const E_TOOL_001: &str = "E-TOOL-001";
/// Error code: tool handler missing for a declared schema.
pub const E_TOOL_002: &str = "E-TOOL-002";

/// Domain tag used on tool registry errors.
pub const DOMAIN_TOOL: &str = "TOOL";

/// Declared tool schema exposed to the model.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolSchema {
    /// Tool name visible to the model.
    pub name: String,
    /// Human readable description.
    pub description: String,
    /// JSON Schema of the tool arguments; must be a JSON object.
    pub parameters: serde_json::Value,
}

impl ToolSchema {
    /// Build a schema, rejecting a non-object parameter document.
    pub fn new(
        name: impl Into<String>,
        description: impl Into<String>,
        parameters: serde_json::Value,
    ) -> Result<Self, ContractError> {
        if !parameters.is_object() {
            return Err(ContractError::here(
                E_TOOL_001,
                DOMAIN_TOOL,
                "tool parameters must be a JSON object",
            ));
        }
        Ok(Self {
            name: name.into(),
            description: description.into(),
            parameters,
        })
    }
}

/// Result of a tool invocation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolOutcome {
    /// Whether the call succeeded.
    pub ok: bool,
    /// Payload returned to the model.
    pub payload: serde_json::Value,
}

impl ToolOutcome {
    /// Build a successful outcome.
    pub fn ok(payload: serde_json::Value) -> Self {
        Self { ok: true, payload }
    }

    /// Build a failed outcome.
    pub fn failed(payload: serde_json::Value) -> Self {
        Self { ok: false, payload }
    }
}

/// One registered tool: its schema plus its handler.
pub struct RegisteredTool {
    schema: ToolSchema,
    handler: Box<dyn Fn(&serde_json::Value) -> ToolOutcome + Send + Sync>,
}

impl RegisteredTool {
    /// Build a registered tool from a schema and a handler.
    pub fn new(
        schema: ToolSchema,
        handler: impl Fn(&serde_json::Value) -> ToolOutcome + Send + Sync + 'static,
    ) -> Self {
        Self {
            schema,
            handler: Box::new(handler),
        }
    }

    /// The declared schema.
    pub fn schema(&self) -> &ToolSchema {
        &self.schema
    }

    /// Invoke the handler.
    pub fn invoke(&self, args: &serde_json::Value) -> ToolOutcome {
        (self.handler)(args)
    }
}

impl std::fmt::Debug for RegisteredTool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RegisteredTool")
            .field("schema", &self.schema)
            .finish_non_exhaustive()
    }
}

/// Service Definition for the tool registry seam.
pub trait ToolRegistryPort: Send + Sync {
    /// Register a tool under its schema name; the disposer revokes it on drop.
    fn register_tool(&self, tool: RegisteredTool) -> Result<Disposer, ContractError>;
    /// Ordered schemas of all registered tools, in registration order.
    fn schemas(&self) -> Vec<ToolSchema>;
    /// Invoke a registered tool by name.
    fn invoke_tool(
        &self,
        name: &str,
        args: &serde_json::Value,
    ) -> Result<ToolOutcome, ContractError>;
}

/// First-phase provider: ordered explicit registry backed by `IndexMap`.
#[derive(Debug, Default)]
pub struct IndexMapToolRegistry {
    registry: SharedRegistry<RegisteredTool>,
}

impl IndexMapToolRegistry {
    /// Build an empty registry.
    pub fn new() -> Self {
        Self {
            registry: SharedRegistry::new("contracts/src/tool_registry.rs"),
        }
    }

    /// Number of registered tools.
    pub fn len(&self) -> usize {
        self.registry.len()
    }

    /// Whether no tool is registered.
    pub fn is_empty(&self) -> bool {
        self.registry.is_empty()
    }
}

impl ToolRegistryPort for IndexMapToolRegistry {
    fn register_tool(&self, tool: RegisteredTool) -> Result<Disposer, ContractError> {
        let key = tool.schema.name.clone();
        self.registry.register(&key, tool)
    }

    fn schemas(&self) -> Vec<ToolSchema> {
        self.registry
            .entries_snapshot()
            .into_iter()
            .map(|(_, tool)| tool.schema().clone())
            .collect()
    }

    fn invoke_tool(
        &self,
        name: &str,
        args: &serde_json::Value,
    ) -> Result<ToolOutcome, ContractError> {
        self.registry
            .require(name, |tool| tool.invoke(args))
            .map_err(|err| {
                ContractError::new(
                    E_TOOL_002,
                    DOMAIN_TOOL,
                    format!("tool handler not registered: {name}"),
                    "contracts/src/tool_registry.rs",
                )
                .with_stderr_tail(err.detail)
            })
    }
}
