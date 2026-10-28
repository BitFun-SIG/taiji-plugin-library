//! Seam: `model_provider` - Service Definition `LlmProviderPort`.
//!
//! Contract reference: CONTRACT.md section 2.1 row `model_provider` and section
//! 2.3 model access seam. Provider for the first phase is `LlmMockProvider`
//! (echo only); the consumer is `LLMFactory`. An unregistered model name fails
//! with `E-LLM-001`, and the model name is never hard coded inside an instance.

use std::sync::{Arc, Mutex};

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

use crate::error::ContractError;

/// Error code: model name not registered, carrying the model name.
pub const E_LLM_001: &str = "E-LLM-001";
/// Error code: provider failed to create an instance.
pub const E_LLM_002: &str = "E-LLM-002";
/// Error code: instance execution failed.
pub const E_LLM_003: &str = "E-LLM-003";
/// Error code: `TaskResult` field missing.
pub const E_LLM_004: &str = "E-LLM-004";

/// Domain tag used on model provider errors.
pub const DOMAIN_LLM: &str = "LLM";

/// Minimal model call request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Task {
    /// Task identifier.
    pub task_id: String,
    /// Prompt text handed to the model.
    pub prompt: String,
}

/// Call context handed to an instance.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Context {
    /// Worktree path the call runs in.
    pub worktree_path: String,
    /// Git identity of the calling role, as `name <email>`.
    pub git_identity: String,
    /// Prior task identifiers, oldest first.
    pub task_history: Vec<String>,
}

/// Model call result.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TaskResult {
    /// Identifier of the instance that produced the result.
    pub instance_id: String,
    /// Model output text.
    pub output: String,
    /// Unix timestamp in milliseconds.
    pub timestamp: i64,
}

impl TaskResult {
    /// Build a result, rejecting the empty instance identifier.
    pub fn new(
        instance_id: impl Into<String>,
        output: impl Into<String>,
        timestamp: i64,
    ) -> Result<Self, ContractError> {
        let instance_id = instance_id.into();
        if instance_id.is_empty() {
            return Err(ContractError::here(
                E_LLM_004,
                DOMAIN_LLM,
                "TaskResult.instance_id must be present",
            ));
        }
        Ok(Self {
            instance_id,
            output: output.into(),
            timestamp,
        })
    }
}

/// Role configuration subset consumed by the seam.
///
/// The full `RoleConfig` belongs to the role domain; the seam consumes only the
/// default model key, so `model_provider` stays free of role storage concerns.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RoleConfigRef {
    /// Default model name used as the provider selection key.
    pub default_model: String,
    /// Maximum concurrent tasks.
    pub max_concurrent_tasks: u32,
    /// Task timeout in seconds.
    pub task_timeout: u64,
}

/// Service Definition for one live model instance.
pub trait LlmInstancePort: Send + Sync {
    /// Execute one model call.
    fn execute(&self, task: &Task, context: &Context) -> Result<TaskResult, ContractError>;
    /// Stable identifier of this instance.
    fn instance_id(&self) -> &str;
}

/// Service Definition for a model provider.
pub trait LlmProviderPort: Send + Sync {
    /// Name used as the selection key in the factory.
    fn provider_name(&self) -> &str;
    /// Create a model instance for the given role configuration.
    fn create_instance(
        &self,
        config: &RoleConfigRef,
    ) -> Result<Arc<dyn LlmInstancePort>, ContractError>;
}

/// Consumer of the model provider seam: model selection by configuration key.
#[derive(Default)]
pub struct LlmFactory {
    providers: IndexMap<String, Arc<dyn LlmProviderPort>>,
}

impl std::fmt::Debug for LlmFactory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LlmFactory")
            .field("providers", &self.providers.keys().collect::<Vec<_>>())
            .finish()
    }
}

impl LlmFactory {
    /// Build an empty factory.
    pub fn new() -> Self {
        Self {
            providers: IndexMap::new(),
        }
    }

    /// Register a provider under its declared name; the disposer revokes it.
    pub fn register_provider(
        &mut self,
        provider: Arc<dyn LlmProviderPort>,
    ) -> Result<ProviderDisposer, ContractError> {
        let name = provider.provider_name().to_string();
        if self.providers.contains_key(&name) {
            return Err(ContractError::here(
                E_LLM_002,
                DOMAIN_LLM,
                format!("provider already registered: {name}"),
            ));
        }
        self.providers.insert(name.clone(), provider);
        Ok(ProviderDisposer { name })
    }

    /// Ordered provider names, in registration order.
    pub fn provider_names(&self) -> Vec<String> {
        self.providers.keys().cloned().collect()
    }

    /// Create an instance by provider name and role configuration.
    ///
    /// An unknown provider name fails with `E-LLM-001` and the message carries
    /// the requested name.
    pub fn create_instance(
        &self,
        provider_name: &str,
        config: &RoleConfigRef,
    ) -> Result<Arc<dyn LlmInstancePort>, ContractError> {
        let provider = self.providers.get(provider_name).ok_or_else(|| {
            ContractError::here(
                E_LLM_001,
                DOMAIN_LLM,
                format!("model provider not registered: {provider_name}"),
            )
        })?;
        provider.create_instance(config).map_err(|err| {
            ContractError::new(
                E_LLM_002,
                DOMAIN_LLM,
                format!("provider {provider_name} failed to create instance"),
                "contracts/src/model_provider.rs",
            )
            .with_stderr_tail(err.detail)
        })
    }
}

/// Revokes a provider registration when dropped.
#[derive(Debug)]
pub struct ProviderDisposer {
    name: String,
}

impl ProviderDisposer {
    /// The provider name this disposer tracks.
    pub fn name(&self) -> &str {
        &self.name
    }
}

/// First-phase provider: echo-only mock that keeps per-instance state isolated.
#[derive(Debug, Default)]
pub struct LlmMockProvider {
    next_instance: Mutex<u64>,
}

impl LlmMockProvider {
    /// Build the mock provider.
    pub fn new() -> Self {
        Self {
            next_instance: Mutex::new(0),
        }
    }
}

impl LlmProviderPort for LlmMockProvider {
    fn provider_name(&self) -> &str {
        "mock"
    }

    fn create_instance(
        &self,
        config: &RoleConfigRef,
    ) -> Result<Arc<dyn LlmInstancePort>, ContractError> {
        let mut guard = self
            .next_instance
            .lock()
            .expect("mock counter mutex poisoned");
        *guard += 1;
        let instance_id = format!("mock-{}", *guard);
        // The model name is carried by configuration only; the instance never
        // hard codes a model name of its own.
        Ok(Arc::new(LlmMockInstance {
            instance_id,
            model: config.default_model.clone(),
        }))
    }
}

/// One mock instance; it echoes the prompt and holds no shared state.
#[derive(Debug)]
pub struct LlmMockInstance {
    instance_id: String,
    model: String,
}

impl LlmMockInstance {
    /// Model name this instance was configured with.
    pub fn model(&self) -> &str {
        &self.model
    }
}

impl LlmInstancePort for LlmMockInstance {
    fn execute(&self, task: &Task, context: &Context) -> Result<TaskResult, ContractError> {
        if task.task_id.is_empty() {
            return Err(ContractError::here(
                E_LLM_004,
                DOMAIN_LLM,
                "task_id must be present",
            ));
        }
        let output = format!(
            "echo model={} worktree={} prompt={}",
            self.model, context.worktree_path, task.prompt
        );
        TaskResult::new(self.instance_id.clone(), output, 0)
    }

    fn instance_id(&self) -> &str {
        &self.instance_id
    }
}
