//! Discovery channel for the D-04 execution face.
//!
//! `src/lib.rs` is owned by the N21 closing seat and is not modified by this
//! seam card, so the new module is not yet declared there. This integration
//! test pulls the module in by path, which exercises the same source file the
//! closing seat will wire into `lib.rs` by name.
#[path = "../src/hook_function_runtime.rs"]
mod hook_function_runtime;

#[test]
fn the_execution_face_is_constructible_and_reports_unavailable() {
    use hook_function_runtime::LnfuHookFunctionRuntime;
    use openbitfun_runtime_ports::{HookFunctionAvailability, HookFunctionRuntime};

    let runtime = LnfuHookFunctionRuntime::new();
    match runtime.availability() {
        HookFunctionAvailability::Unavailable { reason } => {
            assert!(
                reason.contains("not attached"),
                "unexpected reason: {reason}"
            );
        }
        HookFunctionAvailability::Available => {
            panic!("the execution face must not claim availability before a runtime is attached")
        }
    }
}
