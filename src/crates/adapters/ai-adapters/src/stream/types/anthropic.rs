use super::unified::{UnifiedResponse, UnifiedTokenUsage, UnifiedToolCall};
use bitfun_agent_stream::ToolCallCompletion;
use serde::Deserialize;

pub(crate) fn map_anthropic_stop_reason(reason: &str) -> ToolCallCompletion {
    match reason.trim().to_ascii_lowercase().as_str() {
        "tool_use" => ToolCallCompletion::NormalToolUse,
        "max_tokens" => ToolCallCompletion::OutputLimit,
        "refusal" => ToolCallCompletion::Failed,
        "end_turn" | "stop_sequence" | "pause_turn" => ToolCallCompletion::NormalNoToolUse,
        _ => ToolCallCompletion::Unknown,
    }
}

#[derive(Debug, Deserialize)]
pub struct MessageStart {
    pub message: Message,
}

#[derive(Debug, Deserialize)]
pub struct Message {
    pub usage: Option<Usage>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct Usage {
    input_tokens: Option<u32>,
    output_tokens: Option<u32>,
    cache_read_input_tokens: Option<u32>,
    cache_creation_input_tokens: Option<u32>,
}

impl Usage {
    pub fn update(&mut self, other: &Usage) {
        if other.input_tokens.is_some() {
            self.input_tokens = other.input_tokens;
        }
        if other.output_tokens.is_some() {
            self.output_tokens = other.output_tokens;
        }
        if other.cache_read_input_tokens.is_some() {
            self.cache_read_input_tokens = other.cache_read_input_tokens;
        }
        if other.cache_creation_input_tokens.is_some() {
            self.cache_creation_input_tokens = other.cache_creation_input_tokens;
        }
    }

    pub fn is_empty(&self) -> bool {
        self.input_tokens.is_none()
            && self.output_tokens.is_none()
            && self.cache_read_input_tokens.is_none()
            && self.cache_creation_input_tokens.is_none()
    }
}

/// How the wire's `input_tokens` field should be interpreted when folding a
/// provider usage block into the unified token usage.
///
/// - `Native`: real Anthropic semantics — `input_tokens` counts only the
///   uncached portion and is disjoint from `cache_read_input_tokens` /
///   `cache_creation_input_tokens`, so the three must be summed to obtain the
///   total-context "input tokens" metric.
/// - `NonNative`: backend that merely speaks the Anthropic wire format (e.g.
///   Zhipu/GLM behind an Anthropic-compatible gateway). These backends report
///   `input_tokens` as the FULL prompt (already including cached tokens), so
///   adding cache components again would double-count input. Their hit-rate
///   denominator is simply `input_tokens` as reported.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AnthropicUsageSemantics {
    #[default]
    Native,
    NonNative,
}

impl Usage {
    /// Fold this usage into a [`UnifiedTokenUsage`] under the given
    /// interpretation of `input_tokens`. See [`AnthropicUsageSemantics`].
    pub fn into_unified(self, semantics: AnthropicUsageSemantics) -> UnifiedTokenUsage {
        let cache_read = self.cache_read_input_tokens;
        let cache_creation = self.cache_creation_input_tokens;

        let prompt_token_count = match semantics {
            AnthropicUsageSemantics::Native => {
                // prompt_token_count = total context tokens occupied
                // (industry-standard "input tokens" metric). For native
                // Anthropic the three fields are disjoint and must be summed.
                self.input_tokens.unwrap_or(0)
                    + cache_read.unwrap_or(0)
                    + cache_creation.unwrap_or(0)
            }
            AnthropicUsageSemantics::NonNative => {
                // The wire field already covers fresh + cached prompt tokens;
                // keep it verbatim so the downstream hit rate denominator is
                // the full prompt rather than an inflated sum.
                self.input_tokens.unwrap_or(0)
            }
        };
        let candidates_token_count = self.output_tokens.unwrap_or(0);

        UnifiedTokenUsage {
            prompt_token_count,
            candidates_token_count,
            total_token_count: prompt_token_count + candidates_token_count,
            reasoning_token_count: None,
            // cached_content_token_count = cache READS only. This is the
            // numerator for `cache hit rate = cached / prompt`. Writes go
            // to cache_creation_token_count below.
            cached_content_token_count: cache_read,
            cache_creation_token_count: cache_creation,
        }
    }
}

impl From<Usage> for UnifiedTokenUsage {
    fn from(value: Usage) -> Self {
        value.into_unified(AnthropicUsageSemantics::Native)
    }
}

#[derive(Debug, Deserialize)]
pub struct MessageDelta {
    pub delta: MessageDeltaDelta,
    pub usage: Option<Usage>,
}

#[derive(Debug, Deserialize)]
pub struct MessageDeltaDelta {
    pub stop_reason: Option<String>,
    pub stop_sequence: Option<String>,
}

impl MessageDelta {
    pub fn into_unified_response(self, semantics: AnthropicUsageSemantics) -> UnifiedResponse {
        UnifiedResponse {
            text: None,
            reasoning_content: None,
            reasoning_content_kind: None,
            thinking_signature: None,
            tool_call: None,
            usage: self.usage.map(|usage| usage.into_unified(semantics)),
            tool_call_completion: self
                .delta
                .stop_reason
                .as_deref()
                .map(map_anthropic_stop_reason),
            finish_reason: self.delta.stop_reason,
            provider_metadata: None,
            model_response_replay: None,
        }
    }
}

impl From<MessageDelta> for UnifiedResponse {
    fn from(value: MessageDelta) -> Self {
        value.into_unified_response(AnthropicUsageSemantics::Native)
    }
}

#[derive(Debug, Deserialize)]
pub struct ContentBlockStart {
    pub index: Option<usize>,
    pub content_block: ContentBlock,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
pub enum ContentBlock {
    #[serde(rename = "thinking")]
    Thinking {
        thinking: Option<String>,
        signature: Option<String>,
    },
    #[serde(rename = "text")]
    Text,
    #[serde(rename = "tool_use")]
    ToolUse { id: String, name: String },
    #[serde(other)]
    Unknown,
}

impl From<ContentBlockStart> for UnifiedResponse {
    fn from(value: ContentBlockStart) -> Self {
        let mut result = UnifiedResponse::default();
        match value.content_block {
            ContentBlock::ToolUse { id, name } => {
                let tool_call = UnifiedToolCall {
                    tool_call_index: value.index,
                    id: Some(id),
                    name: Some(name),
                    arguments: None,
                    arguments_is_snapshot: false,
                };
                result.tool_call = Some(tool_call);
            }
            ContentBlock::Thinking {
                thinking,
                signature,
            } => {
                result.reasoning_content = thinking;
                result.thinking_signature = signature;
            }
            _ => {}
        }
        result
    }
}

#[derive(Debug, Deserialize)]
pub struct ContentBlockDelta {
    index: Option<usize>,
    delta: Delta,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
pub enum Delta {
    #[serde(rename = "thinking_delta")]
    Thinking { thinking: String },
    #[serde(rename = "text_delta")]
    Text { text: String },
    #[serde(rename = "input_json_delta")]
    InputJson { partial_json: String },
    #[serde(rename = "signature_delta")]
    Signature { signature: String },
    #[serde(other)]
    Unknown,
}

impl TryFrom<ContentBlockDelta> for UnifiedResponse {
    type Error = String;
    fn try_from(value: ContentBlockDelta) -> Result<Self, Self::Error> {
        let mut result = UnifiedResponse::default();
        match value.delta {
            Delta::Thinking { thinking } => {
                result.reasoning_content = Some(thinking);
            }
            Delta::Text { text } => {
                result.text = Some(text);
            }
            Delta::InputJson { partial_json } => {
                let tool_call = UnifiedToolCall {
                    tool_call_index: value.index,
                    id: None,
                    name: None,
                    arguments: Some(partial_json),
                    arguments_is_snapshot: false,
                };
                result.tool_call = Some(tool_call);
            }
            Delta::Signature { signature } => {
                result.thinking_signature = Some(signature);
            }
            Delta::Unknown => {
                return Err("Unsupported anthropic delta type".to_string());
            }
        }
        Ok(result)
    }
}

#[derive(Debug, Deserialize)]
pub struct AnthropicSSEError {
    pub error: AnthropicSSEErrorDetails,
}

#[derive(Debug, Deserialize)]
pub struct AnthropicSSEErrorDetails {
    #[serde(rename = "type")]
    pub error_type: String,
    pub message: String,
}

impl From<AnthropicSSEErrorDetails> for String {
    fn from(value: AnthropicSSEErrorDetails) -> Self {
        format!("{}: {}", value.error_type, value.message)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stream::types::unified::UnifiedTokenUsage;
    use bitfun_agent_stream::ToolCallCompletion;

    #[test]
    fn cached_content_token_count_is_reads_only_not_sum() {
        let raw = r#"{
            "input_tokens": 100,
            "output_tokens": 50,
            "cache_read_input_tokens": 30,
            "cache_creation_input_tokens": 20
        }"#;
        let usage: Usage = serde_json::from_str(raw).expect("valid anthropic usage");
        let unified: UnifiedTokenUsage = usage.into();

        // cached_content_token_count must be reads only — NOT read + creation.
        // This guarantees `cached_content / prompt` is a correct hit rate.
        assert_eq!(unified.cached_content_token_count, Some(30));
        assert_eq!(unified.cache_creation_token_count, Some(20));

        // prompt_token_count keeps "total context" semantic (matches industry
        // standard "input tokens" metric across providers).
        assert_eq!(unified.prompt_token_count, 150);
        assert_eq!(unified.candidates_token_count, 50);
        assert_eq!(unified.total_token_count, 200);

        // Hit rate computed by downstream:
        //   30 / 150 == 20% (correct: only reads count as hits)
        // Pre-fix this would have been wrongly 50/150 == 33%.
    }

    #[test]
    fn non_native_backend_keeps_input_verbatim() {
        // Non-native backends speaking the Anthropic wire format (e.g. Zhipu
        // GLM behind an Anthropic-compatible gateway) report `input_tokens` as
        // the FULL prompt — already including cached tokens. Adding the cache
        // components again would double-count input and halve the displayed
        // hit rate.
        let raw = r#"{
            "input_tokens": 254638,
            "output_tokens": 924,
            "cache_read_input_tokens": 126848,
            "cache_creation_input_tokens": null
        }"#;
        let usage: Usage = serde_json::from_str(raw).expect("valid non-native usage");
        let unified =
            usage.into_unified(AnthropicUsageSemantics::NonNative);

        // prompt stays verbatim: fresh 127790 + cached 126848 = 254638.
        assert_eq!(unified.prompt_token_count, 254_638);
        assert_eq!(unified.candidates_token_count, 924);
        assert_eq!(unified.total_token_count, 255_562);

        // Numerator semantics unchanged: reads only, writes separate.
        assert_eq!(unified.cached_content_token_count, Some(126_848));
        assert_eq!(unified.cache_creation_token_count, None);

        // Downstream hit rate = 126848 / 254638 ≈ 49.8% — matches the vendor's
        // own cached/total view of this sample instead of the inflated 99.7%
        // double-counted reading.
    }

    #[test]
    fn native_semantics_preserved_by_default_ctor_and_explicit_flag() {
        let raw = r#"{
            "input_tokens": 100,
            "output_tokens": 50,
            "cache_read_input_tokens": 30,
            "cache_creation_input_tokens": 20
        }"#;
        let usage: Usage = serde_json::from_str(raw).expect("valid anthropic usage");

        let via_from: UnifiedTokenUsage = usage.clone().into();
        assert_eq!(via_from.prompt_token_count, 150);

        let via_flag = usage.into_unified(AnthropicUsageSemantics::Native);
        assert_eq!(via_flag.prompt_token_count, 150);
    }

    #[test]
    fn maps_anthropic_end_turn_as_normal_no_tool_use() {
        assert_eq!(
            map_anthropic_stop_reason("tool_use"),
            ToolCallCompletion::NormalToolUse
        );
        assert_eq!(
            map_anthropic_stop_reason("max_tokens"),
            ToolCallCompletion::OutputLimit
        );
        assert_eq!(
            map_anthropic_stop_reason("end_turn"),
            ToolCallCompletion::NormalNoToolUse
        );
        assert_eq!(
            map_anthropic_stop_reason("unknown_stop"),
            ToolCallCompletion::Unknown
        );
    }

    #[test]
    fn absent_cache_fields_stay_none() {
        let raw = r#"{ "input_tokens": 100, "output_tokens": 50 }"#;
        let usage: Usage = serde_json::from_str(raw).expect("valid anthropic usage");
        let unified: UnifiedTokenUsage = usage.into();
        assert_eq!(unified.cached_content_token_count, None);
        assert_eq!(unified.cache_creation_token_count, None);
    }

    #[test]
    fn zero_cache_fields_are_some_zero_not_none() {
        // Cache support reported but zero this call must be distinguishable
        // from "provider did not report cache fields at all".
        let raw = r#"{
            "input_tokens": 100,
            "output_tokens": 50,
            "cache_read_input_tokens": 0,
            "cache_creation_input_tokens": 0
        }"#;
        let usage: Usage = serde_json::from_str(raw).expect("valid anthropic usage");
        let unified: UnifiedTokenUsage = usage.into();
        assert_eq!(unified.cached_content_token_count, Some(0));
        assert_eq!(unified.cache_creation_token_count, Some(0));
    }

    #[test]
    fn only_read_present_no_creation() {
        let raw = r#"{
            "input_tokens": 100,
            "output_tokens": 50,
            "cache_read_input_tokens": 30
        }"#;
        let usage: Usage = serde_json::from_str(raw).expect("valid anthropic usage");
        let unified: UnifiedTokenUsage = usage.into();
        assert_eq!(unified.cached_content_token_count, Some(30));
        assert_eq!(unified.cache_creation_token_count, None);
        // prompt_token_count = input + read (no creation contribution)
        assert_eq!(unified.prompt_token_count, 130);
    }
}
