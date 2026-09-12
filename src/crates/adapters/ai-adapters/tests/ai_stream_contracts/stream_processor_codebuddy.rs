use crate::common::stream_test_harness::{run_stream_fixture, StreamFixtureProvider};
use openbitfun_events::{AgenticEvent, ToolEventData};
use serde_json::json;

/// Some OpenAI-compatible providers (e.g. CodeBuddy) emit an empty
/// `finish_reason` string on every tool-call delta. Left untouched, the shared
/// accumulator treats each frame as a terminal signal and finalizes
/// partially-built tool calls (empty arguments -> EOF parse -> error -> retry).
///
/// The seam in `taiji-codebuddy-adapter` drops those degenerate finish reasons
/// so argument fragments accumulate across frames and the tool call is only
/// finalized at the natural stream end. This drives the full
/// handle_openai_stream -> StreamProcessor chain.
///
/// The repair is value-based (empty/whitespace only), so it is active for every
/// OpenAI-compatible provider and needs no per-provider flag or URL match.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn degenerate_empty_finish_reason_frames_accumulate_tool_args() {
    let output = run_stream_fixture(
        StreamFixtureProvider::OpenAi,
        "stream/openai/codebuddy_empty_finish_reason_tool.sse",
        Default::default(),
    )
    .await;

    let result = output.result.expect("stream result");

    // Assertions 1 + 3: the tool call is fully accumulated (raw_len > 0) and
    // parsed into a valid argument object instead of being dropped as an EOF
    // parse error on a zero-length payload.
    assert_eq!(result.tool_calls.len(), 1);
    assert_eq!(result.tool_calls[0].tool_id, "call_cb_1");
    assert_eq!(result.tool_calls[0].tool_name, "get_weather");
    assert_eq!(result.tool_calls[0].arguments, json!({ "city": "Beijing" }));
    assert!(!result.tool_calls[0].is_error);
    let raw_arguments = result.tool_calls[0]
        .raw_arguments
        .as_deref()
        .expect("raw arguments must be preserved for an accumulated tool call");
    assert!(
        !raw_arguments.is_empty(),
        "tool arguments must be accumulated (raw_len > 0), not dropped"
    );
    assert_eq!(raw_arguments, "{\"city\":\"Beijing\"}");

    // Assertion 2: the empty finish_reason frames must not trigger a retry /
    // stream failure. The tool call is finalized exactly once at stream end.
    let failed_or_cancelled = output.events.iter().any(|event| {
        matches!(
            event,
            AgenticEvent::DialogTurnFailed { .. } | AgenticEvent::DialogTurnCancelled { .. }
        )
    });
    assert!(
        !failed_or_cancelled,
        "empty finish_reason frames must not cause a stream failure / retry"
    );

    let params_partial: Vec<&str> = output
        .events
        .iter()
        .filter_map(|event| match event {
            AgenticEvent::ToolEvent {
                tool_event: ToolEventData::ParamsPartial { params, .. },
                ..
            } => Some(params.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(params_partial, vec!["{\"city\":", "\"Beijing\"", "}"]);
}
