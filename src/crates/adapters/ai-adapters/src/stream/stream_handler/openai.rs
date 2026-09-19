use super::inline_think::InlineThinkParser;
use super::stream_stats::StreamStats;
use super::{StreamTimeoutController, StreamTimeoutStage, TimedStreamItem, next_stream_item};
use crate::stream::types::openai::OpenAISSEData;
use crate::stream::types::unified::UnifiedResponse;
use anyhow::{Result, anyhow};
use eventsource_stream::Eventsource;
use futures::StreamExt;
use log::{error, trace, warn};
use openbitfun_core_types::errors::AiProviderError;
use reqwest::Response;
use serde_json::Value;
use std::time::Duration;
use taiji_codebuddy_adapter::normalize_response as normalize_degenerate_finish_reason;
use tokio::sync::mpsc;

const OPENAI_CHAT_COMPLETION_CHUNK_OBJECT: &str = "chat.completion.chunk";
/// MiniMax (and possibly other providers) close a streaming response with a
/// non-streaming `chat.completion` frame instead of a true `chunk`. That final
/// frame is the only one carrying authoritative usage, so we accept it too.
const OPENAI_CHAT_COMPLETION_OBJECT: &str = "chat.completion";
const AI_STREAM_RESPONSE_TARGET: &str = "ai::openai_stream_response";

#[derive(Debug)]
struct OpenAIResponseNormalizer {
    inline_think_parser: InlineThinkParser,
}

impl OpenAIResponseNormalizer {
    fn new(inline_think_in_text: bool) -> Self {
        Self {
            inline_think_parser: InlineThinkParser::new(inline_think_in_text),
        }
    }

    fn normalize_response(&mut self, response: UnifiedResponse) -> Vec<UnifiedResponse> {
        self.inline_think_parser
            .normalize_response(response)
            .into_iter()
            // SEAM (user-side): repair protocol-deviant frames. Value-based, so
            // it needs no provider id / URL match / signature change.
            .map(normalize_degenerate_finish_reason)
            .collect()
    }

    fn flush(&mut self) -> Vec<UnifiedResponse> {
        self.inline_think_parser.flush()
    }
}

fn is_valid_chat_completion_chunk_weak(event_json: &Value) -> bool {
    // Standard streaming frames use `chat.completion.chunk`. MiniMax's final
    // SSE frame, however, switches to the non-streaming `chat.completion`
    // shape (choice carries `message` rather than `delta`) and is the ONLY
    // chunk that contains the authoritative usage block. Accept both — the
    // OpenAISSEData deserialization downstream tolerates either choice shape.
    matches!(
        event_json.get("object").and_then(|value| value.as_str()),
        Some(OPENAI_CHAT_COMPLETION_CHUNK_OBJECT) | Some(OPENAI_CHAT_COMPLETION_OBJECT)
    )
}

fn extract_sse_api_error_message(event_json: &Value) -> Option<String> {
    let error = event_json.get("error")?;
    if let Some(message) = error.get("message").and_then(|value| value.as_str()) {
        return Some(message.to_string());
    }
    if let Some(message) = error.as_str() {
        return Some(message.to_string());
    }
    Some("An error occurred during streaming".to_string())
}

fn extract_sse_api_error(event_json: &Value) -> Option<AiProviderError> {
    let error = event_json.get("error")?;
    let code = error
        .get("code")
        .or_else(|| error.get("type"))
        .and_then(Value::as_str)
        .map(str::to_string);
    let message = extract_sse_api_error_message(event_json)?;
    Some(AiProviderError::from_parts(
        message,
        Some("openai".to_string()),
        code,
        None,
    ))
}

/// Timeout policy shared by the OpenAI stream entry point.
struct OpenAiStreamPolicy {
    ttft_timeout: Option<Duration>,
    idle_timeout: Option<Duration>,
}

/// Convert a byte stream into a structured response stream
///
/// # Arguments
/// * `response` - HTTP response
/// * `tx_event` - parsed event sender
/// * `tx_raw_sse` - optional raw SSE sender (collect raw data for diagnostics)
pub async fn handle_openai_stream(
    response: Response,
    tx_event: mpsc::UnboundedSender<Result<UnifiedResponse>>,
    tx_raw_sse: Option<mpsc::UnboundedSender<String>>,
    inline_think_in_text: bool,
    ttft_timeout: Option<Duration>,
    idle_timeout: Option<Duration>,
) {
    handle_openai_stream_inner(
        response,
        tx_event,
        tx_raw_sse,
        inline_think_in_text,
        OpenAiStreamPolicy {
            ttft_timeout,
            idle_timeout,
        },
    )
    .await
}

async fn handle_openai_stream_inner(
    response: Response,
    tx_event: mpsc::UnboundedSender<Result<UnifiedResponse>>,
    tx_raw_sse: Option<mpsc::UnboundedSender<String>>,
    inline_think_in_text: bool,
    policy: OpenAiStreamPolicy,
) {
    let OpenAiStreamPolicy {
        ttft_timeout,
        idle_timeout,
    } = policy;
    // Temporary Qoder diagnosis: the gateway answers 200 and then closes at once,
    // so the wire shape (content type, transfer encoding, status) is what
    // separates "empty 200" from "frames we failed to parse".
    eprintln!(
        "[qoder-seam] stream entry: status={} content_type={:?} transfer={:?} encoding={:?} content_length={:?}",
        response.status(),
        response.headers().get(reqwest::header::CONTENT_TYPE),
        response.headers().get(reqwest::header::TRANSFER_ENCODING),
        response.headers().get(reqwest::header::CONTENT_ENCODING),
        response.headers().get(reqwest::header::CONTENT_LENGTH),
    );
    // Temporary Qoder diagnosis: observe the raw bytes that reach the SSE parser
    // without changing them, so an empty 200 and a mis-framed stream are told
    // apart by evidence rather than inference.
    let mut raw_chunk_count = 0usize;
    let mut stream = response
        .bytes_stream()
        .map(move |chunk| {
            if let Ok(bytes) = chunk.as_ref() {
                raw_chunk_count += 1;
                if raw_chunk_count <= 3 {
                    let text = String::from_utf8_lossy(bytes);
                    eprintln!(
                        "[qoder-seam] raw chunk #{raw_chunk_count} ({} bytes): {:?}",
                        bytes.len(),
                        text.chars().take(500).collect::<String>()
                    );
                }
            }
            chunk
        })
        .eventsource();
    let mut stats = StreamStats::new("OpenAI");
    let mut timeout_controller = StreamTimeoutController::new(ttft_timeout, idle_timeout);
    // Track whether a chunk with `finish_reason` was received.
    // Some providers (e.g. MiniMax) close the stream after the final chunk
    // without sending `[DONE]`, so we treat `Ok(None)` as a normal termination
    // when a finish_reason has already been seen.
    let mut received_finish_reason = false;
    let mut normalizer = OpenAIResponseNormalizer::new(inline_think_in_text);

    loop {
        let sse = match next_stream_item(&mut stream, &timeout_controller).await {
            TimedStreamItem::Item(Ok(sse)) => sse,
            TimedStreamItem::End => {
                if received_finish_reason {
                    for normalized_response in normalizer.flush() {
                        stats.record_unified_response(&normalized_response);
                        let _ = tx_event.send(Ok(normalized_response));
                    }
                    stats.log_summary("stream_closed_after_finish_reason");
                    return;
                }
                let error_msg = "SSE stream closed before response completed";
                stats.log_summary("stream_closed_before_completion");
                error!("{}", error_msg);
                eprintln!("[qoder-seam] stream closed before any event; see stats above");
                let _ = tx_event.send(Err(anyhow!(error_msg)));
                return;
            }
            TimedStreamItem::Item(Err(e)) => {
                let error_msg = format!("SSE stream error: {}", e);
                stats.log_summary("sse_stream_error");
                error!("{}", error_msg);
                let _ = tx_event.send(Err(anyhow!(error_msg)));
                return;
            }
            TimedStreamItem::TimedOut(StreamTimeoutStage::Ttft) => {
                let timeout_secs = ttft_timeout.map(|timeout| timeout.as_secs()).unwrap_or(0);
                let error_msg = format!(
                    "OpenAI stream TTFT timeout after {}s waiting for first effective output",
                    timeout_secs
                );
                stats.log_summary("ttft_timeout");
                error!("{}", error_msg);
                let _ = tx_event.send(Err(anyhow!(error_msg)));
                return;
            }
            TimedStreamItem::TimedOut(StreamTimeoutStage::Idle) => {
                let timeout_secs = idle_timeout.map(|timeout| timeout.as_secs()).unwrap_or(0);
                let error_msg = format!("SSE stream timeout after {}s", timeout_secs);
                stats.log_summary("sse_stream_timeout");
                error!("{}", error_msg);
                let _ = tx_event.send(Err(anyhow!(error_msg)));
                return;
            }
        };

        let mut raw = sse.data;
        stats.record_sse_event("data");
        trace!(target: AI_STREAM_RESPONSE_TARGET, "OpenAI SSE: {:?}", raw);
        if let Some(ref tx) = tx_raw_sse {
            let _ = tx.send(raw.clone());
        }
        // A user-side seam may answer in its provider's own envelope rather than
        // plain OpenAI frames. The Qoder CN gateway does exactly that — every
        // chunk arrives as `{"headers":…,"body":"<openai chunk>",…}` — so the
        // seam unwraps it first. Non-Qoder traffic never enters this branch, so
        // every other provider stays byte-identical.
        let prefixed = format!("data:{raw}");
        if let Some(unfolded) = taiji_qoder_adapter::unfold_frame(&prefixed) {
            match unfolded {
                taiji_qoder_adapter::UnfoldedFrame::Data(payload) => {
                    raw = payload
                        .strip_prefix("data:")
                        .unwrap_or(&payload)
                        .to_string();
                }
                taiji_qoder_adapter::UnfoldedFrame::Done => {
                    for normalized_response in normalizer.flush() {
                        stats.record_unified_response(&normalized_response);
                        let _ = tx_event.send(Ok(normalized_response));
                    }
                    stats.increment("marker:done");
                    stats.log_summary("done_marker_received");
                    return;
                }
                taiji_qoder_adapter::UnfoldedFrame::Skip => continue,
            }
        }
        if raw == "[DONE]" {
            for normalized_response in normalizer.flush() {
                stats.record_unified_response(&normalized_response);
                let _ = tx_event.send(Ok(normalized_response));
            }
            stats.increment("marker:done");
            stats.log_summary("done_marker_received");
            return;
        }

        let event_json: Value = match serde_json::from_str(&raw) {
            Ok(json) => json,
            Err(e) => {
                let error_msg = format!("SSE parsing error: {}, data: {}", e, &raw);
                stats.increment("error:sse_parsing");
                stats.log_summary("sse_parsing_error");
                error!("{}", error_msg);
                let _ = tx_event.send(Err(anyhow!(error_msg)));
                return;
            }
        };

        if let Some(mut provider_error) = extract_sse_api_error(&event_json) {
            provider_error.message =
                format!("SSE API error: {}, data: {}", provider_error.message, raw);
            stats.increment("error:api");
            stats.log_summary("sse_api_error");
            error!("{}", provider_error);
            let _ = tx_event.send(Err(anyhow!(provider_error)));
            return;
        }

        if !is_valid_chat_completion_chunk_weak(&event_json) {
            stats.increment("skip:non_standard_event");
            warn!(
                "Skipping non-standard OpenAI SSE event; object={}",
                event_json
                    .get("object")
                    .and_then(|value| value.as_str())
                    .unwrap_or("<missing>")
            );
            continue;
        }

        stats.increment("chunk:chat_completion");
        let sse_data: OpenAISSEData = match serde_json::from_value(event_json) {
            Ok(event) => event,
            Err(e) => {
                let error_msg = format!("SSE data schema error: {}, data: {}", e, &raw);
                stats.increment("error:schema");
                stats.log_summary("sse_data_schema_error");
                error!("{}", error_msg);
                let _ = tx_event.send(Err(anyhow!(error_msg)));
                return;
            }
        };

        let tool_call_count = sse_data.first_choice_tool_call_count();
        if tool_call_count > 1 {
            stats.increment("chunk:multi_tool_call");
            warn!(
                "OpenAI SSE chunk contains {} tool calls in the first choice; emitting indexed tool deltas",
                tool_call_count
            );
        }

        let has_empty_choices = sse_data.is_choices_empty();
        let unified_responses = sse_data.into_unified_responses();
        trace!(
            target: AI_STREAM_RESPONSE_TARGET,
            "OpenAI unified responses: {:?}",
            unified_responses
        );
        if unified_responses.is_empty() {
            if has_empty_choices {
                stats.increment("skip:empty_choices_no_usage");
                warn!(
                    "Ignoring OpenAI SSE chunk with empty choices and no usage payload: {}",
                    raw
                );
                // Ignore keepalive/metadata chunks with empty choices and no usage payload.
                continue;
            }
            // Defensive fallback: this should be unreachable if OpenAISSEData::into_unified_responses
            // keeps returning at least one event for all non-empty-choices chunks.
            let error_msg = format!("OpenAI SSE chunk produced no unified events, data: {}", raw);
            stats.increment("error:no_unified_events");
            stats.log_summary("no_unified_events");
            error!("{}", error_msg);
            let _ = tx_event.send(Err(anyhow!(error_msg)));
            return;
        }

        for unified_response in unified_responses {
            let normalized_responses = normalizer.normalize_response(unified_response);
            if normalized_responses.is_empty() {
                continue;
            }

            for normalized_response in normalized_responses {
                timeout_controller.observe_unified_response(&normalized_response);
                if normalized_response.finish_reason.is_some() {
                    received_finish_reason = true;
                }
                stats.record_unified_response(&normalized_response);
                let _ = tx_event.send(Ok(normalized_response));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        OpenAIResponseNormalizer, extract_sse_api_error, extract_sse_api_error_message,
        is_valid_chat_completion_chunk_weak,
    };
    use crate::stream::types::unified::UnifiedResponse;
    use openbitfun_core_types::errors::ErrorCategory;

    // SEAM (user-side): the host-side capability entrance the accessed software
    // calls to hand a capability result back. The body lives outside the
    // upstream git tree (see Cargo.toml, the user-side seal above).
    use lnfu_host_seam::deliver_capability_result;

    #[test]
    fn weak_filter_accepts_chat_completion_chunk() {
        let event = serde_json::json!({
            "object": "chat.completion.chunk"
        });
        assert!(is_valid_chat_completion_chunk_weak(&event));
    }

    /// The Qoder gateway answers every chunk inside its own envelope. Without
    /// unwrapping, the frame carries no `choices` field and the weak filter
    /// drops it, so the stream never yields content. Both halves are asserted:
    /// the raw envelope is rejected, the unwrapped payload is accepted.
    #[test]
    fn qoder_envelope_is_unwrapped_before_the_weak_filter() {
        let envelope = r#"{"headers":{"Content-Type":["application/json"]},"body":"{\"choices\":[{\"delta\":{\"content\":\"P\",\"role\":\"assistant\"},\"index\":0}],\"model\":\"auto\",\"object\":\"chat.completion.chunk\"}","statusCodeValue":200,"statusCode":"OK"}"#;
        let as_openai = serde_json::from_str::<serde_json::Value>(envelope).expect("valid json");
        assert!(
            !is_valid_chat_completion_chunk_weak(&as_openai),
            "the raw envelope must not look like an OpenAI chunk"
        );

        let unfolded = taiji_qoder_adapter::unfold_frame(&format!("data:{envelope}"))
            .expect("the seam recognises its own envelope");
        let taiji_qoder_adapter::UnfoldedFrame::Data(payload) = unfolded else {
            panic!("expected a data frame, got {unfolded:?}");
        };
        let inner = payload.strip_prefix("data:").unwrap_or(&payload);
        let parsed = serde_json::from_str::<serde_json::Value>(inner).expect("inner parses");
        assert!(
            is_valid_chat_completion_chunk_weak(&parsed),
            "the unwrapped payload must pass the weak filter"
        );
    }

    #[test]
    fn weak_filter_rejects_non_standard_object() {
        let event = serde_json::json!({
            "object": ""
        });
        assert!(!is_valid_chat_completion_chunk_weak(&event));
    }

    #[test]
    fn weak_filter_rejects_missing_object() {
        let event = serde_json::json!({
            "id": "chatcmpl_test"
        });
        assert!(!is_valid_chat_completion_chunk_weak(&event));
    }

    #[test]
    fn weak_filter_accepts_minimax_final_chat_completion_object() {
        // MiniMax's last SSE frame uses `chat.completion` (non-streaming shape)
        // instead of `chat.completion.chunk`. That frame carries the only
        // authoritative usage block, so it must NOT be dropped at the gate.
        let event = serde_json::json!({
            "object": "chat.completion",
            "choices": [{"finish_reason": "stop", "index": 0, "message": {}}],
            "usage": {"prompt_tokens": 45, "completion_tokens": 47, "total_tokens": 92}
        });
        assert!(is_valid_chat_completion_chunk_weak(&event));
    }

    #[test]
    fn extracts_api_error_message_from_object_shape() {
        let event = serde_json::json!({
            "error": {
                "message": "provider error"
            }
        });
        assert_eq!(
            extract_sse_api_error_message(&event).as_deref(),
            Some("provider error")
        );
    }

    #[test]
    fn preserves_context_overflow_code_from_stream_error() {
        let event = serde_json::json!({
            "error": {
                "code": "context_length_exceeded",
                "message": "Request failed"
            }
        });

        let error = extract_sse_api_error(&event).expect("provider error");
        assert_eq!(error.category, ErrorCategory::ContextOverflow);
        assert_eq!(
            error.provider_code.as_deref(),
            Some("context_length_exceeded")
        );
    }

    #[test]
    fn extracts_api_error_message_from_string_shape() {
        let event = serde_json::json!({
            "error": "provider error"
        });
        assert_eq!(
            extract_sse_api_error_message(&event).as_deref(),
            Some("provider error")
        );
    }

    #[test]
    fn returns_none_when_no_error_payload_exists() {
        let event = serde_json::json!({
            "object": "chat.completion.chunk"
        });
        assert!(extract_sse_api_error_message(&event).is_none());
    }

    #[test]
    fn non_codebuddy_normalizer_passthrough_unchanged() {
        // Providers that follow the OpenAI contract are unaffected: the seam
        // only rewrites degenerate (empty/whitespace) finish reasons.
        let mut normalizer = OpenAIResponseNormalizer::new(false);
        let responses = normalizer.normalize_response(UnifiedResponse {
            finish_reason: Some("stop".to_string()),
            ..Default::default()
        });
        assert_eq!(responses.len(), 1);
        assert_eq!(responses[0].finish_reason.as_deref(), Some("stop"));
    }

    #[test]
    fn degenerate_empty_finish_reason_is_dropped_through_normalizer() {
        let mut normalizer = OpenAIResponseNormalizer::new(false);
        let responses = normalizer.normalize_response(UnifiedResponse {
            finish_reason: Some("".to_string()),
            ..Default::default()
        });
        assert_eq!(responses.len(), 1);
        assert!(responses[0].finish_reason.is_none());
    }

    #[test]
    fn host_seam_entrance_accepts_a_capability_result() {
        // SEAM (user-side): the accessed software reaches the host-side entrance
        // through the imported name, so the entrance is a callable entry on this
        // side of the seam. A non-empty payload is delivered and reported as a
        // success outcome.
        let outcome = deliver_capability_result("ai-adapters", "normalize-response", "stop")
            .expect("a non-empty payload is delivered");
        assert_eq!(outcome.payload.as_deref(), Some("stop"));
    }
}
