//! LNFU capability seam service definitions.
//!
//! This crate is the lowest layer of the seam module tree: it holds the eight
//! first-phase service definitions (ports) fixed by CONTRACT.md section 2.1 plus
//! the shared registration mechanism of section 2.5. It depends on nothing from
//! the host layer, so plugins can depend on these definitions alone and keep the
//! zero-private-dependency rule of T-02.
//!
//! The eight seams, in contract order:
//!
//! | seam id | Service Definition |
//! |---|---|
//! | `tool_registry` | `ToolRegistryPort` |
//! | `model_provider` | `LlmProviderPort` |
//! | `subagent` | `SubagentPort` |
//! | `system_prompt` | `SystemPromptPort` |
//! | `event_bus` | `RuntimeEventSink` |
//! | `storage` | `StoragePort` |
//! | `config` | `ConfigPort` |
//! | `web_panel` | `WebPanelPort` |

#![forbid(unsafe_code)]

pub mod availability;
pub mod config;
pub mod descriptor;
pub mod error;
pub mod event_bus;
pub mod model_provider;
pub mod registry;
pub mod resource_codes;
pub mod storage;
pub mod subagent;
pub mod system_prompt;
pub mod tool_registry;
pub mod web_panel;

pub use availability::Availability;
pub use config::{
    ConfigPort, ConfigSource, EnvConfigProvider, Resolved, StartupArgs, result_file_name,
};
pub use descriptor::{SeamDescriptor, SeamId};
pub use error::ContractError;
pub use event_bus::{
    BoundedEventBus, EventDomain, JournalSink, RuntimeEventEnvelope, RuntimeEventSink,
    RuntimeEventType,
};
pub use model_provider::{
    Context, LlmFactory, LlmInstancePort, LlmMockInstance, LlmMockProvider, LlmProviderPort,
    ProviderDisposer, RoleConfigRef, Task, TaskResult,
};
pub use registry::{Disposer, ErasedRevoke, OrderedRegistry, RegistryPort, SharedRegistry};
pub use resource_codes::{
    DOMAIN_RESOURCE, E_RES_001, E_RES_002, E_RES_003, E_RES_004, E_RES_005, E_RES_006, E_RES_007,
};
pub use storage::{JsonFileStorage, StoragePort, StorageSnapshot};
pub use subagent::{NullSubagentRuntime, SubagentOutcome, SubagentPort, SubagentSpec};
pub use system_prompt::{PromptSection, StaticPromptProvider, SystemPromptPort};
pub use tool_registry::{
    IndexMapToolRegistry, RegisteredTool, ToolOutcome, ToolRegistryPort, ToolSchema,
};
pub use web_panel::{EmptyWebPanel, PanelResponse, WebPanelPort};
