//! Qoder SSE envelope unfolding (the seam's response-side face).
//!
//! The Qoder CN gateway does not emit plain OpenAI frames. Each SSE frame is a
//! second envelope:
//!
//! ```text
//! data:{"headers":{...},"body":"{\"choices\":[...]}","statusCodeValue":200,"statusCode":"OK"}
//! ```
//!
//! where `body` is the OpenAI chunk the host's stream handler already knows how
//! to parse. The stream also ends with a `[DONE]` sentinel inside its own
//! envelope plus a trailing `event:finish` frame, both of which the host's
//! OpenAI handler would otherwise see as malformed data.
//!
//! Unfolding happens here rather than upstream so no host-side stream handler
//! needs a Qoder branch: the seam hands back ordinary OpenAI frames and every
//! other provider stays untouched.

/// One unfolded frame: the payload the host's OpenAI handler expects, or a
/// control signal it already understands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UnfoldedFrame {
    /// A plain OpenAI payload (`{"choices":[...]}`), ready to forward.
    Data(String),
    /// The terminal sentinel; the caller forwards it and then closes the stream.
    Done,
    /// A frame the host's handler has no use for (gateway bookkeeping such as
    /// `event:finish`); the caller drops it.
    Skip,
}

/// True when `line` looks like a Qoder envelope rather than a plain frame.
///
/// A Qoder envelope always carries `"statusCode"` alongside a string `body`;
/// plain OpenAI frames carry `"choices"` at the top level.
pub fn is_qoder_envelope(line: &str) -> bool {
    let Some(payload) = line.strip_prefix("data:") else {
        return false;
    };
    let payload = payload.trim();
    if !payload.starts_with('{') {
        return false;
    }
    payload.contains("\"statusCode\"") && payload.contains("\"body\"")
}

/// Unfolds one Qoder SSE frame into the payload the host's OpenAI handler wants.
///
/// Returns `None` when the input is not a Qoder envelope at all, letting a
/// caller pass the original line through unchanged.
pub fn unfold_frame(line: &str) -> Option<UnfoldedFrame> {
    let payload = line.strip_prefix("data:")?.trim();
    if !is_qoder_envelope(line) {
        return None;
    }
    let value: serde_json::Value = serde_json::from_str(payload).ok()?;
    // A non-2xx status means the gateway rejected the request; surface it as a
    // plain frame so the existing error path sees the gateway's own message.
    if let Some(status) = value.get("statusCodeValue").and_then(|v| v.as_u64()) {
        if !(200..300).contains(&status) {
            return Some(UnfoldedFrame::Data(payload.to_string()));
        }
    }
    let body = value.get("body").and_then(|v| v.as_str())?;
    let body = body.trim();
    if body == "[DONE]" {
        return Some(UnfoldedFrame::Done);
    }
    if body.is_empty() {
        return Some(UnfoldedFrame::Skip);
    }
    Some(UnfoldedFrame::Data(format!("data:{body}")))
}

/// Queue-admission hint carried by a Qoder rate-limit envelope.
///
/// The gateway answers an admitted-but-throttled inference with an HTTP 200
/// whose first SSE frame is a `403` envelope carrying the official CLI's own
/// queue contract:
///
/// ```text
/// {"code":"403","message":"{\"code\":\"10605\",\"message\":
///   \"{\\\"isQueued\\\":false,\\\"modelKey\\\":\\\"degrade\\\",
///      \\\"retryAfterSeconds\\\":5,...}\"}"}
/// ```
///
/// The official CLI reacts by waiting `retryAfterSeconds` and replaying the
/// same request (see its `checkModelQueueStatus` / `ask/finish` calls). The
/// signal lives inside the SSE body, not in an HTTP `Retry-After` header, so a
/// transport that only inspects headers never sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueueRetryHint {
    /// Seconds the gateway asked the client to wait before replaying.
    pub retry_after_seconds: u64,
    /// Gateway queue class (`degrade`, `p3`, ...) when the frame carries one.
    pub model_key: Option<String>,
}

/// Extracts the queue-retry hint from a Qoder throttling envelope.
///
/// Returns `None` for every frame that is not a queue rejection, so a caller
/// keeps its ordinary error path for all other failures. Handles the gateway's
/// nested-JSON encoding (the queue payload is a JSON document embedded as a
/// string inside a JSON document).
pub fn queue_retry_hint(line: &str) -> Option<QueueRetryHint> {
    let payload = line.strip_prefix("data:")?.trim();
    if !payload.starts_with('{') {
        return None;
    }
    let outer: serde_json::Value = serde_json::from_str(payload).ok()?;
    // Only a non-2xx envelope can carry a queue rejection.
    let status = outer.get("statusCodeValue").and_then(|v| v.as_u64())?;
    if (200..300).contains(&status) {
        return None;
    }
    // The queue payload hides behind the gateway's nested string encoding: the
    // envelope carries it in `body`, whose JSON string nests `message` several
    // levels deep. Peel one string at a time until the queue fields appear.
    let mut candidate = outer
        .get("body")
        .or_else(|| outer.get("message"))
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    for _ in 0..6 {
        if let Some(retry) = candidate.get("retryAfterSeconds").and_then(|v| v.as_u64()) {
            return Some(QueueRetryHint {
                retry_after_seconds: retry,
                model_key: candidate
                    .get("modelKey")
                    .and_then(|v| v.as_str())
                    .map(str::to_string),
            });
        }
        let text = candidate.as_str()?;
        let parsed = serde_json::from_str::<serde_json::Value>(text).ok()?;
        candidate = parsed
            .get("message")
            .cloned()
            .unwrap_or_else(|| parsed.clone());
    }
    None
}

/// Warns when a frame carries a Qoder envelope this module cannot unfold, so a
/// gateway protocol change surfaces in the log instead of silently yielding a
/// stream that never produces content.
pub fn envelope_is_unfamiliar(line: &str) -> bool {
    line.strip_prefix("data:")
        .map(str::trim)
        .filter(|payload| payload.starts_with('{'))
        .is_some_and(|payload| payload.contains("\"statusCode\"") && !payload.contains("\"body\""))
}

#[cfg(test)]
mod tests {
    use super::*;

    const FRAME_P: &str = r#"data:{"headers":{"Content-Type":["application/json"]},"body":"{\"choices\":[{\"delta\":{\"content\":\"P\",\"role\":\"assistant\"},\"index\":0}],\"model\":\"auto\",\"object\":\"chat.completion.chunk\"}","statusCodeValue":200,"statusCode":"OK"}"#;

    #[test]
    fn unfolds_a_content_frame_into_a_plain_openai_frame() {
        let unfolded = unfold_frame(FRAME_P).expect("frame is a qoder envelope");
        match unfolded {
            UnfoldedFrame::Data(payload) => {
                assert!(payload.starts_with("data:{"));
                assert!(payload.contains("\"choices\""));
                assert!(payload.contains("\"content\":\"P\""));
                // The outer envelope must be gone, not merely wrapped.
                assert!(!payload.contains("statusCode"));
            }
            other => panic!("expected Data, got {other:?}"),
        }
    }

    #[test]
    fn folds_the_terminal_sentinel_into_done() {
        let frame = r#"data:{"headers":{"Content-Type":["application/json"]},"body":"[DONE]","statusCodeValue":200,"statusCode":"OK"}"#;
        assert_eq!(unfold_frame(frame), Some(UnfoldedFrame::Done));
    }

    #[test]
    fn skips_empty_bookkeeping_frames() {
        let frame = r#"data:{"headers":{},"body":"","statusCodeValue":200,"statusCode":"OK"}"#;
        assert_eq!(unfold_frame(frame), Some(UnfoldedFrame::Skip));
    }

    #[test]
    fn leaves_plain_openai_frames_alone() {
        let plain = r#"data:{"choices":[{"delta":{"content":"hi"}}]}"#;
        assert_eq!(unfold_frame(plain), None);
        assert!(!is_qoder_envelope(plain));
        // A non-data line (event:, blank) is never an envelope either.
        assert_eq!(unfold_frame("event:finish"), None);
        assert_eq!(unfold_frame(""), None);
    }

    #[test]
    fn surfaces_a_non_2xx_envelope_for_the_error_path() {
        let frame = r#"data:{"body":"quota exceeded","statusCodeValue":429,"statusCode":"Too Many Requests"}"#;
        match unfold_frame(frame) {
            Some(UnfoldedFrame::Data(payload)) => assert!(payload.contains("quota exceeded")),
            other => panic!("expected Data, got {other:?}"),
        }
    }

    #[test]
    fn flags_an_envelope_shape_change() {
        // Same envelope marker (a `statusCode` field), but no `body` field: the
        // module cannot unfold it, so the shape change must be reported.
        let shifted = r#"data:{"statusCode":"OK","statusCodeValue":200,"payload":"{}"}"#;
        assert!(envelope_is_unfamiliar(shifted));
        assert!(!envelope_is_unfamiliar(FRAME_P));
    }

    /// Real frame captured from the installed Qoder CN CLI 1.1.57 run log
    /// (`~/.qoder-cn/logs/runs/2026-09-19T16-17-27-372+08-00-r2yii1-p17224/qodercli.log`),
    /// where the CLI replayed the request 5s later and succeeded.
    const QUEUE_REJECTION: &str = r#"data:{"body":"{\"code\":\"403\",\"message\":\"{\\\"code\\\":\\\"10605\\\",\\\"message\\\":\\\"{\\\\\\\"isQueued\\\\\\\":false,\\\\\\\"modelKey\\\\\\\":\\\\\\\"degrade\\\\\\\",\\\\\\\"queueCount\\\\\\\":0,\\\\\\\"queueType\\\\\\\":\\\\\\\"p3\\\\\\\",\\\\\\\"retryAfterSeconds\\\\\\\":5,\\\\\\\"serviceAvailable\\\\\\\":true,\\\\\\\"waitTime\\\\\\\":0}\\\"}\"}","statusCodeValue":403,"statusCode":"FORBIDDEN"}"#;

    #[test]
    fn extracts_the_queue_retry_hint_from_a_throttling_envelope() {
        let hint = queue_retry_hint(QUEUE_REJECTION).expect("queue rejection yields a hint");
        assert_eq!(hint.retry_after_seconds, 5);
        assert_eq!(hint.model_key.as_deref(), Some("degrade"));
    }

    #[test]
    fn a_successful_frame_carries_no_queue_hint() {
        assert_eq!(queue_retry_hint(FRAME_P), None);
        let done = r#"data:{"headers":{},"body":"[DONE]","statusCodeValue":200,"statusCode":"OK"}"#;
        assert_eq!(queue_retry_hint(done), None);
    }

    #[test]
    fn a_non_queue_error_carries_no_queue_hint() {
        // A quota rejection is a hard failure: no retry seconds, no replay.
        let quota =
            r#"data:{"body":"quota exceeded","statusCodeValue":403,"statusCode":"FORBIDDEN"}"#;
        assert_eq!(queue_retry_hint(quota), None);
        // A plain (non-envelope) frame is never a hint.
        assert_eq!(queue_retry_hint(r#"data:{"choices":[]}"#), None);
        assert_eq!(queue_retry_hint(""), None);
    }
}
