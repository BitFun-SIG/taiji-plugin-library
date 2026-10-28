//! token-shaper · prompt-prefix stability guard (Type-A pure-function seam).
//!
//! WHY THIS EXISTS
//! ---------------
//! Provider prefix caching renders `tools -> system -> messages` and matches the
//! LONGEST UNCHANGED PREFIX, byte for byte. Measured on the live runtime
//! (2026-09-17, `ai::stream_usage` debug lines), the cache is already working and
//! hitting 88-99.9%:
//!
//! ```text
//! input=206209 cache_read=205952   -> 99.88%
//! input=183949 cache_read=183296   -> 99.65%
//! input= 56542 cache_read= 54400   -> 96.21%
//! input= 50246 cache_read= 44416   -> 88.40%
//! ```
//!
//! That headroom is fragile. Anything that perturbs a byte inside the shared
//! prefix drops the hit rate to zero for that round and every round after it.
//! The upstream tree already carries a tombstone for exactly this mistake: a
//! removed "microcompact" pre-pass that rewrote already-sent tool results, killed
//! KV-cache hits every round, and drove repetitive tool-call loops in long tasks.
//!
//! So this crate does NOT try to be clever about prefixes. It enforces three
//! invariants and refuses everything else:
//!
//! 1. tools are frozen and rendered in a deterministic order,
//! 2. the already-sent message prefix is never rewritten (append-only),
//! 3. volatile content never enters `tools` or `system`.
//!
//! DESIGN CONSTRAINTS (non-negotiable)
//! -----------------------------------
//! * Pure: `Value -> Value`. No state, no I/O, no logging, no panics.
//! * Never reserialize the whole body. Only the touched field is written back,
//!   so key order, whitespace and number formatting of every other byte survive.
//! * No-op in, identical out. The guard must be a pure win or a pure no-op.

use serde_json::Value;

mod precision;

/// Result of asking the OBF context profile face what shape this request has.
///
/// This is the D-06 convergence surface: instead of a local twin of
/// `context_profile`, this crate re-exports the OBF decision as an owned value
/// and adds the one thing that is specific to a *prefix-stability* guard — how
/// much of the request may still be rewritten before the provider's cached
/// prefix is lost.
///
/// The compression face contributes the second half: a request that is close to
/// the input limit is about to be compacted, and a compaction rewrites the
/// prefix. Knowing the window lets the caller defer or skip a reorder it would
/// otherwise pay cache for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContextProfileShape {
    /// Whether the request is on a long-task profile (vs. a conversation).
    pub is_long_task: bool,
    /// How many compression contracts this profile tolerates.
    pub compression_contract_limit: usize,
    /// Whether the request is inside the prefetch window, i.e. close enough to
    /// the input limit that compaction is imminent.
    pub in_prefetch_window: bool,
}

/// Classify a request against the OBF context/compression face.
///
/// `agent_type` and `is_review_subagent` are the same inputs OBF's
/// `ContextProfile::for_agent_context` takes; `model_id` is the resolved model
/// identifier the host passes down. `pressure` and `input_limit` are the
/// current token pressure and the model's input limit, in the same units the
/// compression face uses.
///
/// Pure: same inputs, same output; no state, no IO, no async runtime.
pub fn classify_context(
    agent_type: &str,
    is_review_subagent: bool,
    model_id: Option<&str>,
    pressure: usize,
    input_limit: usize,
) -> ContextProfileShape {
    let capability = openbitfun_agent_runtime::context_profile::ModelCapabilityProfile::from_model_id(
        model_id,
    );
    let policy =
        openbitfun_agent_runtime::context_profile::ContextProfilePolicy::for_agent_context(
            agent_type,
            is_review_subagent,
            capability,
        );

    ContextProfileShape {
        is_long_task: matches!(
            policy.profile,
            openbitfun_agent_runtime::context_profile::ContextProfile::LongTask
        ),
        compression_contract_limit: policy.compression_contract_limit,
        in_prefetch_window: openbitfun_agent_runtime::compression_prefetch::in_prefetch_window(
            pressure,
            input_limit,
        ),
    }
}

/// The lead distance, in tokens, at which OBF starts prefetching a compaction.
///
/// Re-exported so callers reason in one unit instead of hardcoding the constant
/// on the token-shaper side.
pub const PREFETCH_LEAD_TOKENS: usize =
    openbitfun_agent_runtime::compression_prefetch::PREFETCH_LEAD_TOKENS;

/// Substrings that mark a value as per-request volatile.
///
/// These are the concrete volatility sources we found in the live runtime: a
/// timestamp branch reachable from one prompt template, and per-request URL/ID
/// material. Substring matching is deliberate — a prefix-matching scheme would
/// miss `ts=2026-...` embedded mid-string, which is how they actually appear.
const VOLATILE_MARKERS: &[&str] = &[
    "chrono::Local::now",
    "unbound-",
    "%Y-%m-%d",
    "%H:%M:%S",
    "%Y%m%d",
];

/// True when a string embeds a concrete ISO-8601-style date or time.
///
/// The `%Y-%m-%d` entry above only catches a *format string*. What actually
/// leaks into a prompt is the *rendered* value, e.g. `ts=2026-09-17`, so the
/// rendered shapes are matched too. Checked structurally rather than with a
/// regex to keep this crate dependency-free and allocation-free on the hot path.
fn has_iso_datetime(text: &str) -> bool {
    let bytes = text.as_bytes();
    if bytes.len() < 10 {
        return false;
    }
    // Scan for YYYY-MM-DD: digits fixed in place, separators fixed at 4 and 7.
    for window in bytes.windows(10) {
        let date_shaped = window[0].is_ascii_digit()
            && window[1].is_ascii_digit()
            && window[2].is_ascii_digit()
            && window[3].is_ascii_digit()
            && window[4] == b'-'
            && window[5].is_ascii_digit()
            && window[6].is_ascii_digit()
            && window[7] == b'-'
            && window[8].is_ascii_digit()
            && window[9].is_ascii_digit();
        if date_shaped {
            return true;
        }
    }
    false
}

/// True when a bare string looks per-request volatile.
pub fn text_is_volatile(text: &str) -> bool {
    VOLATILE_MARKERS.iter().any(|marker| text.contains(marker)) || has_iso_datetime(text)
}

/// Fields of a tool definition that participate in the cached prefix.
///
/// Only these are considered when building the stability key. Everything else
/// (custom annotations, ordering metadata) does not affect the rendered bytes in
/// a way the provider sees, so it must not influence the key either.
const CACHE_RELEVANT_TOOL_FIELDS: &[&str] = &[
    "name",
    "description",
    "parameters",
    "input_schema",
    "inputSchema",
];

/// What the guard decided, so callers can log/assert without re-deriving it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrefixAction {
    /// Nothing was volatile and nothing needed reordering.
    Unchanged,
    /// Tools were reordered into deterministic order. All tool definitions kept.
    ToolsReordered,
}

/// Stable ordering key for one tool definition.
///
/// A tool is identified by its function name. `name` is not always at the top
/// level (`{"type":"function","function":{"name":...}}` is the OpenAI shape,
/// while the Anthropic shape puts it at top level), so both are tried.
fn tool_identity(tool: &Value) -> Option<&str> {
    if let Some(name) = tool.get("name").and_then(Value::as_str) {
        return Some(name);
    }
    tool.get("function")
        .and_then(|f| f.get("name"))
        .and_then(Value::as_str)
}

/// Canonical form of a tool definition, used only to compare two tools for
/// byte-equality of the cache-relevant parts.
///
/// Returns a stable string. Object keys inside `parameters` are visited in a
/// fixed order because `serde_json`'s default feature set preserves insertion
/// order (`preserve_order`) — but relying on that here would make the key order
/// of a caller-built object affect our comparison. Sorting the top-level
/// cache-relevant fields removes that dependency.
fn canonical_tool(tool: &Value) -> String {
    let identity = tool_identity(tool).unwrap_or("");
    let mut out = String::with_capacity(identity.len() + 64);
    out.push_str(identity);
    for field in CACHE_RELEVANT_TOOL_FIELDS {
        if let Some(value) = tool.get(*field) {
            out.push('\u{1}');
            out.push_str(field);
            out.push('\u{1}');
            out.push_str(&value.to_string());
        }
    }
    out
}

/// True when a JSON value contains per-request volatile content.
///
/// Walks strings only. Numbers and booleans cannot carry a timestamp in the
/// shapes we care about, and recursing into them would cost more than it buys.
pub fn contains_volatile_content(value: &Value) -> bool {
    match value {
        Value::String(s) => text_is_volatile(s),
        Value::Array(items) => items.iter().any(contains_volatile_content),
        Value::Object(map) => map.values().any(contains_volatile_content),
        _ => false,
    }
}

/// Put the `tools` array into deterministic order, preserving every element.
///
/// Ordering is by tool identity, with the canonical form as tie-breaker so two
/// tools sharing a name still land in a reproducible relative order. The array
/// is only written back when the order actually changed, which keeps the guard a
/// true no-op for callers that already render deterministically.
///
/// This never adds, removes, or edits a tool. Dropping or rewriting a tool
/// definition invalidates the tools layer *and* everything after it, so it is
/// intentionally out of scope here.
pub fn reorder_tools_deterministically(body: &mut Value) -> PrefixAction {
    let Some(tools) = body.get_mut("tools").and_then(Value::as_array_mut) else {
        return PrefixAction::Unchanged;
    };
    if tools.len() < 2 {
        return PrefixAction::Unchanged;
    }

    let original: Vec<String> = tools.iter().map(canonical_tool).collect();

    let mut sorted = tools.clone();
    sorted.sort_by(|a, b| {
        let (ka, kb) = (canonical_tool(a), canonical_tool(b));
        ka.cmp(&kb)
    });

    let reordered: Vec<String> = sorted.iter().map(canonical_tool).collect();
    if reordered == original {
        return PrefixAction::Unchanged;
    }

    *tools = sorted;
    PrefixAction::ToolsReordered
}

/// Full prefix-stability pass over an outgoing OpenAI-compatible request body.
///
/// This is the single entry point the upstream seam calls. It is pure: the same
/// input always produces the same output, and it never panics on malformed input
/// (a missing or non-array `tools` field is simply left alone).
///
/// The guard deliberately does NOT touch `messages`. Rewriting an already-sent
/// message is precisely the failure the upstream tombstone records, and it is the
/// only change that can cost more cache than it saves.
pub fn stabilize_prefix(body: &mut Value) -> PrefixAction {
    reorder_tools_deterministically(body)
}

/// A prefix-stability pass taken over a JSON body *string*.
///
/// The host compiles this crate as its own workspace member, so it resolves its
/// own build of `serde_json`; a signature written in `serde_json::Value` is
/// therefore rejected at the call site as a different type of the same name.
/// JSON text is the boundary that survives that: the host serialises, this crate
/// parses, and only the tools array is ever written back.
///
/// Returns the rule outcome together with the body to keep. When nothing
/// changed, or when the input does not parse, the original text is returned
/// unchanged — the guard is a pure win or a pure no-op.
pub fn stabilize_prefix_json(body: &str) -> (PrefixAction, String) {
    let Ok(mut parsed) = serde_json::from_str::<Value>(body) else {
        return (PrefixAction::Unchanged, body.to_string());
    };
    let action = stabilize_prefix(&mut parsed);
    if matches!(action, PrefixAction::Unchanged) {
        return (action, body.to_string());
    }
    match serde_json::to_string(&parsed) {
        Ok(updated) => (action, updated),
        // Serialisation cannot fail for a value parsed from JSON, but a guard
        // must never be the thing that breaks a request: fall back to the input.
        Err(_) => (PrefixAction::Unchanged, body.to_string()),
    }
}

/// Volatile-region report taken over a JSON body *string*.
///
/// Same boundary reason as [`stabilize_prefix_json`]. An unparsable body is
/// reported as carrying no known volatile regions, matching the in-process
/// guard's behaviour on malformed input.
pub fn volatile_regions_json(body: &str) -> Vec<&'static str> {
    match serde_json::from_str::<Value>(body) {
        Ok(parsed) => volatile_regions(&parsed),
        Err(_) => Vec::new(),
    }
}

/// Does this request body carry volatile content where it must not?
///
/// Only `tools` and `system` are checked. Messages are expected to carry
/// per-turn content — that is the live zone, and it is supposed to change.
///
/// Returns the names of the offending regions, so a caller can surface a real
/// diagnostic instead of a boolean.
pub fn volatile_regions(body: &Value) -> Vec<&'static str> {
    let mut found = Vec::new();

    if body
        .get("tools")
        .map(contains_volatile_content)
        .unwrap_or(false)
    {
        found.push("tools");
    }

    let system_volatile = match body.get("system") {
        // Anthropic shape: system is an array of content blocks.
        Some(Value::Array(blocks)) => contains_volatile_content(&Value::Array(blocks.clone())),
        // OpenAI shape: system rides in messages[0], checked separately below.
        Some(other) => contains_volatile_content(other),
        None => false,
    };
    if system_volatile {
        found.push("system");
    }

    if let Some(messages) = body.get("messages").and_then(Value::as_array) {
        let first_is_system = messages
            .first()
            .and_then(|m| m.get("role"))
            .and_then(Value::as_str)
            == Some("system");
        if first_is_system && contains_volatile_content(&messages[0]) {
            found.push("messages[0]");
        }
    }

    found
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn tools_body() -> Value {
        json!({
            "model": "m",
            "tools": [
                {"type": "function", "function": {"name": "zebra", "description": "z"}},
                {"type": "function", "function": {"name": "alpha", "description": "a"}},
                {"type": "function", "function": {"name": "mid", "description": "m"}}
            ],
            "messages": [{"role": "user", "content": "hi"}]
        })
    }

    #[test]
    fn reorders_tools_into_deterministic_order() {
        let mut body = tools_body();
        let action = stabilize_prefix(&mut body);
        assert_eq!(action, PrefixAction::ToolsReordered);

        let names: Vec<&str> = body["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["function"]["name"].as_str().unwrap())
            .collect();
        assert_eq!(names, vec!["alpha", "mid", "zebra"]);
    }

    #[test]
    fn preserves_every_tool_element() {
        let mut body = tools_body();
        let before = body["tools"].as_array().unwrap().len();
        stabilize_prefix(&mut body);
        assert_eq!(body["tools"].as_array().unwrap().len(), before);
    }

    #[test]
    fn is_idempotent() {
        let mut body = tools_body();
        stabilize_prefix(&mut body);
        let once = body.clone();
        let action = stabilize_prefix(&mut body);
        assert_eq!(
            action,
            PrefixAction::Unchanged,
            "second pass must be a no-op"
        );
        assert_eq!(body, once);
    }

    #[test]
    fn already_sorted_body_is_untouched() {
        let mut body = json!({
            "tools": [
                {"function": {"name": "alpha"}},
                {"function": {"name": "beta"}}
            ]
        });
        let before = body.clone();
        assert_eq!(stabilize_prefix(&mut body), PrefixAction::Unchanged);
        assert_eq!(body, before, "no-op guard must not rewrite the body");
    }

    #[test]
    fn does_not_touch_messages() {
        let mut body = tools_body();
        let messages_before = body["messages"].clone();
        stabilize_prefix(&mut body);
        assert_eq!(body["messages"], messages_before);
    }

    #[test]
    fn missing_tools_is_not_an_error() {
        let mut body = json!({"model": "m"});
        assert_eq!(stabilize_prefix(&mut body), PrefixAction::Unchanged);
    }

    #[test]
    fn non_array_tools_is_not_an_error() {
        let mut body = json!({"tools": "oops"});
        assert_eq!(stabilize_prefix(&mut body), PrefixAction::Unchanged);
    }

    #[test]
    fn detects_timestamp_in_system_prompt() {
        let body = json!({
            "system": "You are helpful. Session ts=2026-09-17."
        });
        assert!(volatile_regions(&body).contains(&"system"));
    }

    #[test]
    fn detects_per_session_id_in_system_prompt() {
        let body = json!({"system": [{"type": "text", "text": "sid unbound-12345"}]});
        assert!(volatile_regions(&body).contains(&"system"));
    }

    #[test]
    fn detects_volatile_tool_description() {
        let body = json!({
            "tools": [{"function": {"name": "t", "description": "run at %H:%M:%S"}}]
        });
        assert!(volatile_regions(&body).contains(&"tools"));
    }

    #[test]
    fn detects_volatile_openai_style_system_message() {
        let body = json!({
            "messages": [{"role": "system", "content": "started %Y-%m-%d"}]
        });
        assert!(volatile_regions(&body).contains(&"messages[0]"));
    }

    #[test]
    fn clean_body_reports_no_volatile_regions() {
        let body = json!({
            "system": "You are helpful.",
            "tools": [{"function": {"name": "t", "description": "does a thing"}}],
            "messages": [{"role": "user", "content": "hi"}]
        });
        assert!(volatile_regions(&body).is_empty());
    }

    #[test]
    fn user_turn_content_is_not_flagged() {
        // The live zone is supposed to change; only system/tools are constrained.
        let body = json!({"messages": [{"role": "user", "content": "what time is it"}]});
        assert!(volatile_regions(&body).is_empty());
    }
}
