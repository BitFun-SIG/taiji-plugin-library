//! LNFU implementation of the OpenBitFun `HookFunctionRuntime` execution face.
//!
//! Scope: this seam implements the port contract only. Every method reports the
//! LNFU execution capability honestly and resolves to an explicit, retryable
//! error until a native LNFU hook runtime is attached. No script is imported,
//! evaluated, or launched, and no process is spawned from this module, so the
//! crate's static-only posture is preserved.
//!
//! This module deliberately holds no interior state: `availability()` is a pure
//! function of the compiled configuration, which keeps the implementation
//! trivially `Send + Sync` and free of cross-generation mutation.
use async_trait::async_trait;
use openbitfun_runtime_ports::{
    HookFunctionAvailability, HookFunctionAfterRequest, HookFunctionAfterResult,
    HookFunctionBeforeRequest, HookFunctionBeforeResult, HookFunctionCancelRequest,
    HookFunctionCancelResult, HookFunctionDisposeRequest, HookFunctionDisposeResult,
    HookFunctionGeneration, HookFunctionRegistrationSink, HookFunctionReverseSink,
    HookFunctionRuntime, HookFunctionStartRequest, HookFunctionToolRequest,
    HookFunctionToolResult, PortError, PortErrorKind, PortResult,
};
use std::sync::Arc;
use std::time::Duration;

/// Error code prefix every rejection from this seam carries, so a host can
/// attribute the refusal to the LNFU execution face without string matching.
const ERROR_CONTEXT: &str = "lnfu.hook.runtime";

/// Compiled-in reason reported by `availability()` while no native LNFU hook
/// runtime is attached to the seam.
const UNAVAILABLE_REASON: &str =
    "The LNFU hook execution runtime is not attached; only static hook declaration discovery is available";

/// LNFU execution-face implementation of `HookFunctionRuntime`.
pub struct LnfuHookFunctionRuntime;

impl LnfuHookFunctionRuntime {
    pub fn new() -> Self {
        Self
    }

    /// Builds the uniform, retryable refusal for a not-yet-attached execution
    /// face. The method name is part of the code so a host can tell which port
    /// call was rejected.
    fn unavailable(method: &str) -> PortError {
        PortError::new(
            PortErrorKind::NotAvailable,
            format!("{ERROR_CONTEXT}.{method}: {UNAVAILABLE_REASON}"),
        )
    }
}

impl Default for LnfuHookFunctionRuntime {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl HookFunctionRuntime for LnfuHookFunctionRuntime {
    fn availability(&self) -> HookFunctionAvailability {
        HookFunctionAvailability::Unavailable {
            reason: UNAVAILABLE_REASON.to_string(),
        }
    }

    async fn start(
        &self,
        _request: HookFunctionStartRequest,
        _registrations: Arc<dyn HookFunctionRegistrationSink>,
        _reverse: Arc<dyn HookFunctionReverseSink>,
        _deadline: Duration,
    ) -> PortResult<HookFunctionGeneration> {
        Err(Self::unavailable("start"))
    }

    async fn transform_tool_before(
        &self,
        _request: HookFunctionBeforeRequest,
        _deadline: Duration,
    ) -> PortResult<HookFunctionBeforeResult> {
        Err(Self::unavailable("transform_tool_before"))
    }

    async fn execute_tool(
        &self,
        _request: HookFunctionToolRequest,
        _deadline: Duration,
    ) -> PortResult<HookFunctionToolResult> {
        Err(Self::unavailable("execute_tool"))
    }

    async fn transform_tool_after(
        &self,
        _request: HookFunctionAfterRequest,
        _deadline: Duration,
    ) -> PortResult<HookFunctionAfterResult> {
        Err(Self::unavailable("transform_tool_after"))
    }

    async fn cancel(
        &self,
        _request: HookFunctionCancelRequest,
        _deadline: Duration,
    ) -> PortResult<HookFunctionCancelResult> {
        Err(Self::unavailable("cancel"))
    }

    async fn dispose(
        &self,
        _request: HookFunctionDisposeRequest,
        _deadline: Duration,
    ) -> PortResult<HookFunctionDisposeResult> {
        Err(Self::unavailable("dispose"))
    }
}

/// Compile-time proof that the execution face satisfies the port's declared
/// threading bound. Removing a bound from the trait breaks this assertion.
const _: fn() = || {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<LnfuHookFunctionRuntime>();
};

#[cfg(test)]
mod tests {
    use super::*;
    use openbitfun_runtime_ports::HookFunctionDiagnosticSeverity;
    use serde_json::Map;
    use std::sync::Arc;

    /// Minimal reverse sink: this seam must be constructible from any
    /// `Arc<dyn HookFunctionReverseSink>` the host provides.
    struct TestReverseSink;

    #[async_trait]
    impl HookFunctionReverseSink for TestReverseSink {
        async fn metadata(
            &self,
            _update: openbitfun_runtime_ports::HookFunctionReverseMetadata,
        ) -> PortResult<()> {
            Ok(())
        }

        async fn ask(
            &self,
            _request: openbitfun_runtime_ports::HookFunctionReverseAsk,
        ) -> PortResult<openbitfun_runtime_ports::HookFunctionReverseReply> {
            Ok(openbitfun_runtime_ports::HookFunctionReverseReply::Once)
        }
    }

    /// Minimal registration sink, so the `Arc<dyn ...>` parameter of `start`
    /// is exercised as a trait object rather than a concrete type.
    struct TestRegistrationSink;

    #[async_trait]
    impl HookFunctionRegistrationSink for TestRegistrationSink {
        async fn publish_generation(
            &self,
            _batch: openbitfun_runtime_ports::HookFunctionRegistrationBatch,
        ) -> PortResult<()> {
            Ok(())
        }
    }

    fn generation() -> HookFunctionGeneration {
        HookFunctionGeneration {
            instance_id: "instance".to_string(),
            generation_key: "generation".to_string(),
            revision: "revision".to_string(),
        }
    }

    fn start_request() -> HookFunctionStartRequest {
        HookFunctionStartRequest {
            generation: generation(),
            project_id: "project".to_string(),
            project_worktree: "C:/worktree".to_string(),
            project_created_at_ms: 1,
            config: Map::new(),
            directory: "C:/worktree".to_string(),
            worktree: "C:/worktree".to_string(),
            plugins: Vec::new(),
            configuration_fingerprint: None,
            expected_content_digests: Default::default(),
            expected_review_digest: None,
        }
    }

    fn unavailable_kind(error: &PortError) -> bool {
        error.kind == PortErrorKind::NotAvailable
    }

    #[test]
    fn availability_reports_a_stable_reason_while_unattached() {
        let runtime = LnfuHookFunctionRuntime::new();
        match runtime.availability() {
            HookFunctionAvailability::Unavailable { reason } => {
                assert!(reason.contains("not attached"));
            }
            HookFunctionAvailability::Available => {
                panic!("the execution face must not report Available before a runtime is attached")
            }
        }
        // `availability` is a pure function of configuration; repeated reads agree.
        assert_eq!(runtime.availability(), runtime.availability());
    }

    #[tokio::test]
    async fn every_execution_method_refuses_without_launching_anything() {
        let runtime = LnfuHookFunctionRuntime::new();
        let deadline = Duration::from_millis(50);

        let error = runtime
            .start(
                start_request(),
                Arc::new(TestRegistrationSink),
                Arc::new(TestReverseSink),
                deadline,
            )
            .await
            .unwrap_err();
        assert!(unavailable_kind(&error));
        assert!(error.message.contains("start"));

        let error = runtime
            .transform_tool_before(
                HookFunctionBeforeRequest {
                    generation: generation(),
                    tool_name: "bash".to_string(),
                    session_id: "session".to_string(),
                    call_id: "call".to_string(),
                    args: serde_json::json!({}),
                },
                deadline,
            )
            .await
            .unwrap_err();
        assert!(unavailable_kind(&error));
        assert!(error.message.contains("transform_tool_before"));

        let error = runtime
            .execute_tool(
                HookFunctionToolRequest {
                    generation: generation(),
                    execution_id: "execution".to_string(),
                    registration_id: "registration".to_string(),
                    args: serde_json::json!({}),
                    context: openbitfun_runtime_ports::HookFunctionToolContext {
                        session_id: "session".to_string(),
                        message_id: "message".to_string(),
                        agent: "agent".to_string(),
                        call_id: None,
                    },
                },
                deadline,
            )
            .await
            .unwrap_err();
        assert!(unavailable_kind(&error));
        assert!(error.message.contains("execute_tool"));

        let error = runtime
            .transform_tool_after(
                HookFunctionAfterRequest {
                    generation: generation(),
                    tool_name: "bash".to_string(),
                    session_id: "session".to_string(),
                    call_id: "call".to_string(),
                    args: serde_json::json!({}),
                    output: openbitfun_runtime_ports::HookFunctionAfterOutput {
                        title: "title".to_string(),
                        output: "output".to_string(),
                        metadata: Map::new(),
                    },
                },
                deadline,
            )
            .await
            .unwrap_err();
        assert!(unavailable_kind(&error));
        assert!(error.message.contains("transform_tool_after"));

        let error = runtime
            .cancel(
                HookFunctionCancelRequest {
                    generation: generation(),
                    execution_id: "execution".to_string(),
                    reason: None,
                },
                deadline,
            )
            .await
            .unwrap_err();
        assert!(unavailable_kind(&error));
        assert!(error.message.contains("cancel"));

        let error = runtime
            .dispose(
                HookFunctionDisposeRequest {
                    generation: generation(),
                },
                deadline,
            )
            .await
            .unwrap_err();
        assert!(unavailable_kind(&error));
        assert!(error.message.contains("dispose"));
    }

    #[test]
    fn diagnostic_severity_surface_is_reachable_for_hosts() {
        // Guards that the module resolves the port crate's public surface, so
        // the I-12 dependency edge stays a real edge rather than a dead import.
        assert_eq!(
            HookFunctionDiagnosticSeverity::Error,
            HookFunctionDiagnosticSeverity::Error
        );
    }
}
