//! User-side stream adapter (OUTSIDE the upstream git tree).
//!
//! Purpose: repair `finish_reason: ""` frames emitted by CodeBuddy.
//!
//! Why unconditional: the OpenAI wire contract only ever carries `null` or a
//! documented terminal reason on a terminal frame. An empty / whitespace-only
//! string is therefore a protocol deviation, never a legitimate terminal
//! signal, for ANY OpenAI-compatible provider. Cleaning it unconditionally
//! needs no provider id, no URL match, and no kernel signature change.

use std::collections::HashMap;
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Mutex, OnceLock,
};
use std::time::{SystemTime, UNIX_EPOCH};

use openbitfun_agent_stream::UnifiedResponse;
use openbitfun_core_types::AIConfig;

/// Host suffix every CodeBuddy request targets; kept in sync with the single
/// value-level seam in the upstream openai chat builder.
pub const CODEBUDDY_HOST: &str = "copilot.tencent.com";

/// Lowercase hex alphabet used to render the conversation request id.
const HEX: &[u8; 16] = b"0123456789abcdef";

/// Official CodeBuddy CLI version this adapter impersonates on the wire.
/// Bumped in step with the official CLI (installed CLI baseline: 2.151.0).
const CLI_VERSION: &str = "2.151.0";

/// Fixed CLI disguise values; identity headers are never defaulted.
const DEFAULT_INTENT: &str = "craft";
const DEFAULT_IDE_TYPE: &str = "CLI";
const DEFAULT_IDE_NAME: &str = "CLI";
const DEFAULT_IDE_VERSION: &str = CLI_VERSION;
const DEFAULT_USER_AGENT: &str = "CLI/2.151.0 CodeBuddy/2.151.0";
const DEFAULT_AGENT_PURPOSE: &str = "conversation";
const DEFAULT_AGENT_TYPE: &str = "main";
const DEFAULT_REQUESTED_WITH: &str = "XMLHttpRequest";
const DEFAULT_PRODUCT: &str = "SaaS";
const DEFAULT_PRIVATE_DATA: &str = "true";
const ANONYMOUS_USER_PREFIX: &str = "anonymous_";

/// X-Stainless-* telemetry set captured from the official CLI runtime.
const STAINLESS_ARCH: &str = "x64";
const STAINLESS_LANG: &str = "js";
const STAINLESS_OS: &str = "Windows";
const STAINLESS_PACKAGE_VERSION: &str = "6.25.0";
const STAINLESS_RETRY_COUNT: &str = "0";
const STAINLESS_RUNTIME: &str = "node";
const STAINLESS_RUNTIME_VERSION: &str = "v25.2.1";

/// Identity headers only ever come from user configuration (absent-only on
/// the anonymous wire channel; login state is injected via custom_headers).
const IDENTITY_HEADER_NAMES: [&str; 6] = [
    "X-Userinfo",
    "X-Enterprise-Id",
    "X-Tenant-Id",
    "X-Department-Info",
    "X-Id-Source",
    "X-Auth-Method",
];

/// Whether `url` targets [`CODEBUDDY_HOST`] exactly.
///
/// Anchored host comparison, mirroring the upstream gate in the openai chat
/// builder (which parses the URL and compares `host_str()`); a substring test
/// would also match a hostile URL that merely embeds the host in its path,
/// query, userinfo, or as a suffix domain. Implemented without a URL crate so
/// this adapter keeps its dependency set and cycles unchanged.
///
/// Credential policy on the host side hangs off this predicate: the gateway
/// authenticates with a COSY signature whose header family already carries
/// `Authorization: Bearer COSY.<jwt>`, so the host must suppress its ordinary
/// `Bearer <api-key>` for this host. Emitting both would leave two
/// `Authorization` values on one request (`RequestBuilder::header` appends),
/// which the gateway answers with 400.
pub fn is_codebuddy_url(url: &str) -> bool {
    let trimmed = url.trim();
    // Strip the scheme; without one there is no URL to anchor.
    let Some((_, after_scheme)) = trimmed.split_once("://") else {
        return false;
    };
    // Strip the query and fragment, which are not part of the authority.
    let authority_end = after_scheme
        .find(['/', '?', '#'])
        .unwrap_or(after_scheme.len());
    let authority = &after_scheme[..authority_end];
    // A userinfo section (anything before the last `@`) is attacker-controlled
    // and must never be read as the host, so keep only what follows it.
    let host_port = authority
        .rsplit_once('@')
        .map(|(_, host_port)| host_port)
        .unwrap_or(authority);
    // Drop the port and the root-dot form; IPv6 literals are never the host here.
    let host = host_port
        .rsplit_once(':')
        .map(|(host, _)| host)
        .unwrap_or(host_port);
    host.trim_end_matches('.') == CODEBUDDY_HOST
}

/// Look up a header override without treating whitespace as a real value.
fn configured_value<'a>(
    custom_headers: Option<&'a HashMap<String, String>>,
    name: &str,
) -> Option<&'a str> {
    custom_headers
        .and_then(|headers| headers.get(name))
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
}

/// Process-pinned anonymous identity: the first request generates the values,
/// every later request reuses them. The official CLI keeps conversation id
/// and user id constant within a session; per-request regeneration would
/// defeat provider prefix caches bucketed by user/conversation. The
/// conversation request id is derived per turn from the request body (see
/// [`codebuddy_turn_request_id`]); the pinned value is the fallback used when
/// the body cannot be parsed.
fn pinned_anonymous_identity() -> &'static AnonymousIdentity {
    static IDENTITY: OnceLock<AnonymousIdentity> = OnceLock::new();
    IDENTITY.get_or_init(AnonymousIdentity::generate)
}

/// Turn-keyed state for the request-body-derived conversation request id.
///
/// The upstream seam only hands over the serialized request body, so the turn
/// boundary is observed from the body itself: the first user message of
/// `messages` is the user prompt of the turn, and every later request of that
/// same turn (tool follow-ups, retries, stream re-issues) repeats it verbatim.
/// A different first user message means the next user turn started, so a new
/// id is minted.
///
/// The map is keyed by that fingerprint because several conversations share
/// this one process-wide singleton and interleave their requests. A single
/// "last turn" slot would let one conversation evict another's entry on every
/// interleaved request, so each conversation's follow-up would look like a
/// brand-new turn and split the backend usage-aggregation group again. Keying
/// by fingerprint keeps every in-flight turn independent.
fn turn_state() -> &'static Mutex<TurnStateMap> {
    static STATE: OnceLock<Mutex<TurnStateMap>> = OnceLock::new();
    STATE.get_or_init(|| Mutex::new(TurnStateMap::default()))
}

/// Upper bound on tracked in-flight turns.
///
/// Bounded so a long process cannot grow the map without limit. Eviction is
/// insertion-ordered (see [`TurnStateMap::remember`]); entries are refreshed
/// on every hit, so a live turn is never the oldest candidate for long.
const MAX_TRACKED_TURNS: usize = 256;

/// Insertion-ordered map from a turn fingerprint to the per-turn values handed
/// to the provider for that turn.
#[derive(Default)]
struct TurnStateMap {
    order: Vec<String>,
    turns: HashMap<String, TurnState>,
}

/// Values that stay constant for every request of one user turn: the request
/// group id, the trace id, and the parent span id. The official CLI keeps all
/// three stable across a turn (captured: `captured-multi.jsonl`), while the
/// span id keeps identifying the individual request and is therefore minted
/// per call instead of living here.
#[derive(Clone)]
struct TurnState {
    conversation_request_id: String,
    trace_id: String,
    parent_span_id: String,
}

impl TurnState {
    /// Mint a fresh turn state: a UUIDv7 request-group id plus the 32-hex trace
    /// id and 16-hex parent span id that the trace propagation set reuses for
    /// the whole turn.
    fn generate() -> Self {
        Self {
            conversation_request_id: uuid_v7(),
            trace_id: random_request_id(),
            parent_span_id: random_hex(16),
        }
    }
}

impl TurnStateMap {
    /// Return the state already bound to `fingerprint`, or mint and bind a new
    /// one. A hit also refreshes insertion order so active turns survive
    /// eviction.
    fn remember(&mut self, fingerprint: String, mint: impl FnOnce() -> TurnState) -> TurnState {
        if let Some(existing) = self.turns.get(&fingerprint) {
            let existing = existing.clone();
            self.touch(&fingerprint);
            return existing;
        }
        let state = mint();
        self.turns.insert(fingerprint.clone(), state.clone());
        self.order.push(fingerprint);
        self.evict_overflow();
        state
    }

    /// Move `fingerprint` to the most-recent position.
    fn touch(&mut self, fingerprint: &str) {
        if let Some(pos) = self.order.iter().position(|key| key == fingerprint) {
            let key = self.order.remove(pos);
            self.order.push(key);
        }
    }

    /// Drop the oldest entries once the bound is exceeded.
    fn evict_overflow(&mut self) {
        while self.order.len() > MAX_TRACKED_TURNS {
            let oldest = self.order.remove(0);
            self.turns.remove(&oldest);
        }
    }
}

/// Anonymous defaults for `X-Conversation-Id` / `X-Conversation-Request-ID` /
/// `X-User-Id` / the trace group, fixed per process.
struct AnonymousIdentity {
    conversation_id: String,
    conversation_request_id: String,
    user_id: String,
    trace_id: String,
    parent_span_id: String,
}

impl AnonymousIdentity {
    fn generate() -> Self {
        Self {
            // Official X-Conversation-Id shape is UUIDv7 (48-bit ms
            // timestamp prefix); v4 has no time semantics.
            conversation_id: uuid_v7(),
            conversation_request_id: random_request_id(),
            user_id: format!("{ANONYMOUS_USER_PREFIX}{}", random_hex(8)),
            trace_id: random_request_id(),
            parent_span_id: random_hex(16),
        }
    }
}

/// `len` lowercase hex chars of entropy from the wall clock plus an atomic
/// counter, so consecutive calls on the same millisecond still differ without
/// new deps.
///
/// The state is an LCG whose low bits have a short period: taking the low
/// nibble directly per position made `out[i] == out[i + 16]` always hold
/// (period-16 collision, half the entropy wasted). Fixed by shifting the
/// whole state by a per-position amount before taking the low nibble, then
/// folding the counter in, so every nibble draws from a different slice of
/// the state and no two positions 16 apart coincide.
fn random_hex(len: usize) -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos() as u64)
        .unwrap_or(0);
    let counter = COUNTER.fetch_add(1, Ordering::Relaxed);
    let mut state = nanos
        ^ counter
        ^ (counter << 32)
        ^ (nanos.rotate_left(17).wrapping_mul(0x9E37_79B9_7F4A_7C15));
    let mut chars = vec![b'0'; len];
    for (index, byte) in chars.iter_mut().enumerate() {
        state = state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        // Shift the full state by a position-dependent amount before taking
        // the low nibble, then fold the counter in. This keeps positions 16
        // apart from sharing the same low-bit slice of the LCG.
        let shifted = state >> ((index % 13) * 5);
        let mixed = (shifted & 0x0F) ^ ((counter >> (index % 16 * 4)) & 0x0F);
        *byte = HEX[mixed as usize & 0x0F];
    }
    String::from_utf8(chars).expect("hex digits are ASCII")
}

/// 32 hex chars, used for request ids and trace ids.
fn random_request_id() -> String {
    random_hex(32)
}

/// RFC 9562 version-7 UUID rendered from the shared hex entropy stream:
/// 48-bit big-endian millisecond Unix timestamp followed by random bits,
/// with the version nibble pinned to `7` and the variant nibble in `[89ab]`.
/// Matches the official CLI id shape (48-bit ms timestamp prefix).
fn uuid_v7() -> String {
    let hex = random_hex(32);
    let chars: Vec<char> = hex.chars().collect();
    let join = |range: std::ops::Range<usize>| chars[range].iter().collect::<String>();
    let variant = ["8", "9", "a", "b"][(chars[16] as usize) % 4];
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0);
    let timestamp = format!("{millis:012x}");
    let mut out = String::with_capacity(36);
    // 48-bit timestamp fills groups 1+2 (8 hex + dash + 4 hex).
    out.push_str(&timestamp[0..8]);
    out.push('-');
    out.push_str(&timestamp[8..12]);
    // Group 3: version nibble `7` + 3 random hex; group 4: variant + 3 hex.
    out.push_str("-7");
    out.push_str(&join(0..3));
    out.push('-');
    out.push_str(variant);
    out.push_str(&join(3..6));
    out.push('-');
    out.push_str(&join(6..18));
    out
}

/// Extract the serialized content of the first `user` entry of `messages`.
///
/// Returns `None` when the payload does not expose a usable first user
/// message (not JSON, no `messages` array, no `user` role, non-string
/// content), which is the "cannot judge the turn" signal.
fn first_user_message_fingerprint(request_body: &str) -> Option<String> {
    let body: serde_json::Value = serde_json::from_str(request_body).ok()?;
    let messages = body.get("messages")?.as_array()?;
    let content = messages
        .iter()
        .find(|message| message.get("role").and_then(|role| role.as_str()) == Some("user"))?
        .get("content")?;
    // Content is either a plain string or an array of typed parts; render the
    // whole value so either shape yields a stable comparison key.
    Some(content.to_string())
}

/// Resolve the turn-stable request-group id for one outgoing request.
///
/// `request_body` is the serialized request the upstream seam is about to
/// send. The returned id is stable for every request of one user turn
/// (retries and tool follow-ups included) and changes when the user starts a
/// new turn. A UUIDv7 is minted per turn; when the body cannot be parsed the
/// process-pinned anonymous conversation request id is returned so the caller
/// always receives a usable value.
pub fn codebuddy_turn_request_id(request_body: &str) -> String {
    turn_state_for(request_body).conversation_request_id
}

/// Resolve the turn-stable trace group for one outgoing request.
///
/// Returns `(trace_id, parent_span_id)`, both constant for every request of
/// one user turn, judged from the same request-body fingerprint as
/// [`codebuddy_turn_request_id`] so the trace group and the request-group id
/// share one turn boundary and one lifecycle. When the body cannot be parsed
/// the process-pinned values are returned, matching the fallback the
/// conversation request id uses.
fn codebuddy_turn_trace(request_body: &str) -> (String, String) {
    let state = turn_state_for(request_body);
    (state.trace_id, state.parent_span_id)
}

/// Resolve the turn-stable state for `request_body`, falling back to the
/// process-pinned anonymous identity when the body cannot define a turn.
fn turn_state_for(request_body: &str) -> TurnState {
    let Some(fingerprint) = first_user_message_fingerprint(request_body) else {
        let anonymous = pinned_anonymous_identity();
        return TurnState {
            conversation_request_id: anonymous.conversation_request_id.clone(),
            trace_id: anonymous.trace_id.clone(),
            parent_span_id: anonymous.parent_span_id.clone(),
        };
    };
    let mut state = turn_state()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    state.remember(fingerprint, TurnState::generate)
}

/// Build the full CodeBuddy CLI disguise header set for one request.
///
/// Pure value-level seam input: takes the provider config, target URL, and the
/// serialized request body the caller resolved the turn-stable request-group id
/// from, and returns ready-to-apply header pairs, so the upstream caller needs
/// no reqwest/AIClient dependency here (which would create a dependency cycle).
///
/// Precedence: user `custom_headers` override the CLI defaults; the turn id
/// feeds the conversation scope; the request body feeds the turn-stable trace
/// group (which stays on the process-pinned values when the body cannot define
/// a turn); fixed values stay fixed; identity headers are only emitted when the
/// user configured them.
/// Build the CodeBuddy header family from the host facts this seam needs, in
/// the seam's own types.
///
/// The host compiles this crate as a separate workspace member, so it resolves
/// its own build of `openbitfun_core_types`; a signature written in `AIConfig`
/// is therefore rejected at the call site as a different type of the same name.
/// Only two fields are ever read — `custom_headers` and `api_key` — so those
/// two cross the boundary and the host keeps its struct.
pub fn codebuddy_request_headers_from(
    custom_headers: Option<&std::collections::HashMap<String, String>>,
    api_key: &str,
    url: &str,
    turn_request_id: Option<&str>,
    request_body: &str,
) -> Vec<(&'static str, String)> {
    let config = AIConfig {
        name: String::new(),
        base_url: String::new(),
        request_url: String::new(),
        api_key: api_key.to_string(),
        model: String::new(),
        format: String::new(),
        context_window: 0,
        max_tokens: None,
        max_images_per_request: None,
        temperature: None,
        top_p: None,
        inline_think_in_text: false,
        custom_headers: custom_headers.cloned(),
        custom_headers_mode: None,
        skip_ssl_verify: false,
        custom_request_body: None,
        custom_request_body_mode: None,
    };
    codebuddy_request_headers(&config, url, turn_request_id, request_body)
}

pub fn codebuddy_request_headers(
    config: &AIConfig,
    url: &str,
    turn_request_id: Option<&str>,
    request_body: &str,
) -> Vec<(&'static str, String)> {
    // The caller gates on the CodeBuddy host; assert that contract in debug
    // builds so the header set is never emitted for unrelated providers. The
    // assertion uses the same anchored comparison the upstream gate uses, so it
    // is never weaker than the gate it guards.
    debug_assert!(is_codebuddy_url(url));
    let custom = config.custom_headers.as_ref();
    // Wire order mirrors captured-multi.jsonl (official CLI 2.148.0), with the
    // header set unchanged through the installed 2.151.0 CLI.
    let mut headers: Vec<(&'static str, String)> = Vec::with_capacity(40);

    // --- transport + stainless telemetry ---
    headers.push(("X-Requested-With", DEFAULT_REQUESTED_WITH.to_string()));
    headers.push(("X-Stainless-Arch", STAINLESS_ARCH.to_string()));
    headers.push(("X-Stainless-Lang", STAINLESS_LANG.to_string()));
    headers.push(("X-Stainless-Os", STAINLESS_OS.to_string()));
    headers.push((
        "X-Stainless-Package-Version",
        STAINLESS_PACKAGE_VERSION.to_string(),
    ));
    headers.push(("X-Stainless-Retry-Count", STAINLESS_RETRY_COUNT.to_string()));
    headers.push(("X-Stainless-Runtime", STAINLESS_RUNTIME.to_string()));
    headers.push((
        "X-Stainless-Runtime-Version",
        STAINLESS_RUNTIME_VERSION.to_string(),
    ));

    // --- conversation / agent identity ---
    let anonymous = pinned_anonymous_identity();
    headers.push((
        "X-Conversation-Id",
        configured_value(custom, "X-Conversation-Id")
            .unwrap_or(&anonymous.conversation_id)
            .to_string(),
    ));
    headers.push(("X-Agent-Intent", DEFAULT_INTENT.to_string()));
    headers.push(("X-Agent-Purpose", DEFAULT_AGENT_PURPOSE.to_string()));
    headers.push(("X-IDE-Type", DEFAULT_IDE_TYPE.to_string()));
    headers.push((
        "X-IDE-Name",
        configured_value(custom, "X-IDE-Name")
            .unwrap_or(DEFAULT_IDE_NAME)
            .to_string(),
    ));
    headers.push((
        "X-IDE-Version",
        configured_value(custom, "X-IDE-Version")
            .unwrap_or(DEFAULT_IDE_VERSION)
            .to_string(),
    ));
    headers.push(("X-CodeBuddy-Request", "1".to_string()));
    // Added by official CLI 2.148.0 (absent from the 2.147.0 capture); still
    // present on the installed 2.151.0 CLI, carrying `true` on every request.
    headers.push(("X-Private-Data", DEFAULT_PRIVATE_DATA.to_string()));
    headers.push(("X-Api-Key", config.api_key.clone()));
    // Conversation request id: backend usage-stats aggregation key. Official
    // semantics are turn-level stable (one value across every request of a
    // user turn, retries included), so the caller passes the id resolved by
    // [`codebuddy_turn_request_id`], with the process-pinned fallback for
    // callers that cannot judge the turn; custom headers keep their
    // override-everything semantics. It does not feed any cache prefix (cache
    // routing keys off X-Conversation-Id). X-Request-ID and
    // X-Conversation-Message-ID stay per-request unique.
    let conversation_request_id = configured_value(custom, "X-Conversation-Request-ID")
        .map(str::to_string)
        .or_else(|| {
            turn_request_id
                .map(str::to_string)
                .filter(|turn_id| !turn_id.is_empty())
        })
        .unwrap_or_else(|| anonymous.conversation_request_id.clone());
    // Per capture: X-Request-ID and X-Conversation-Message-ID carry the same
    // per-request value.
    let request_id = random_request_id();
    headers.push(("X-Request-ID", request_id.clone()));
    headers.push(("X-Conversation-Message-ID", request_id));
    // Per capture: X-Root-Request-Id mirrors X-Conversation-Request-ID.
    headers.push(("X-Root-Request-Id", conversation_request_id.clone()));
    headers.push(("X-Conversation-Request-ID", conversation_request_id));
    headers.push(("X-Agent-Type", DEFAULT_AGENT_TYPE.to_string()));

    // --- trace propagation set (W3C traceparent + B3 multi-header) ---
    // Official CLI semantics (captured: `captured-multi.jsonl`, three requests
    // of one turn): the trace id and the parent span id are constant within a
    // turn, while the span id identifies the individual request and changes
    // every time. The turn-stable pair comes from the same turn fingerprint as
    // the request-group id above, so one turn yields one trace; custom headers
    // keep their override-everything semantics.
    let (trace_id, parent_span_id) = codebuddy_turn_trace(request_body);
    let trace_id = configured_value(custom, "X-B3-TraceId")
        .unwrap_or(&trace_id)
        .to_string();
    let parent_span_id = configured_value(custom, "X-B3-ParentSpanId")
        .unwrap_or(&parent_span_id)
        .to_string();
    let span_id = random_hex(16);
    headers.push(("traceparent", format!("00-{trace_id}-{span_id}-01")));
    headers.push(("b3", format!("{trace_id}-{span_id}-1-{parent_span_id}")));
    headers.push(("X-B3-TraceId", trace_id.clone()));
    headers.push(("X-B3-ParentSpanId", parent_span_id));
    headers.push(("X-B3-SpanId", span_id));
    headers.push(("X-B3-Sampled", "1".to_string()));
    headers.push(("X-Trace-Id", trace_id));

    // --- user identity (anonymous default, custom-overridable) ---
    headers.push((
        "X-User-Id",
        configured_value(custom, "X-User-Id")
            .unwrap_or(&anonymous.user_id)
            .to_string(),
    ));
    headers.push(("X-Product", DEFAULT_PRODUCT.to_string()));
    headers.push((
        "User-Agent",
        configured_value(custom, "User-Agent")
            .unwrap_or(DEFAULT_USER_AGENT)
            .to_string(),
    ));

    // --- login-state identity headers: absent-only on the anonymous wire ---
    for name in IDENTITY_HEADER_NAMES {
        if let Some(value) = configured_value(custom, name) {
            headers.push((name, value.to_string()));
        }
    }

    headers
}

/// Whether a response's terminal fact must be dropped as degenerate.
///
/// The seam never names the host's `UnifiedResponse` or `ToolCallCompletion`
/// types. The host compiles this crate as a separate workspace member, so it
/// resolves its own instances of the crates below; a signature written in those
/// types is rejected at the call site as a different type of the same name.
/// The whole repair reduces to one decision over a borrowed `str`, so that is
/// what crosses the boundary — the caller owns the two field assignments.
///
/// An empty finish reason maps to `ToolCallCompletion::Unknown` upstream and
/// tags the frame as terminal, which makes the shared accumulator finalize
/// half-built tool calls on every delta. Both fields must therefore be dropped
/// so the stream continues.
pub fn drops_degenerate_finish_reason(finish_reason: Option<&str>) -> bool {
    finish_reason
        .is_some_and(|reason| reason.trim().is_empty())
}

/// Apply the CodeBuddy stream repair to one unified response.
///
/// Idempotent and provider-neutral: responses without a degenerate
/// `finish_reason` are returned untouched.
pub fn normalize_response(mut response: UnifiedResponse) -> UnifiedResponse {
    if drops_degenerate_finish_reason(response.finish_reason.as_deref()) {
        response.finish_reason = None;
        response.tool_call_completion = None;
    }
    response
}

#[cfg(test)]
mod tests {
    use super::codebuddy_request_headers;
    use super::codebuddy_turn_request_id;
    use super::first_user_message_fingerprint;
    use super::normalize_response;
    use openbitfun_agent_stream::{ToolCallCompletion, UnifiedResponse};
    use openbitfun_core_types::AIConfig;
    use std::collections::HashMap;
    use std::sync::{Mutex, MutexGuard, OnceLock};

    /// Serializes tests that touch the process-global turn state
    /// (`turn_state()` is a `OnceLock<Mutex<TurnStateMap>>` shared by the whole
    /// test binary). Without this, `cargo test` runs them in parallel and one
    /// test's insert can interleave with another's, so a turn-stable assertion
    /// can observe a map mutated by a concurrent test. Tests that call
    /// `codebuddy_turn_request_id` take this guard for their whole body; the
    /// guard recovers from poisoning so a prior panic cannot cascade.
    fn turn_state_guard() -> MutexGuard<'static, ()> {
        static SERIAL: OnceLock<Mutex<()>> = OnceLock::new();
        SERIAL
            .get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn config_with(custom_headers: Option<HashMap<String, String>>) -> AIConfig {
        AIConfig {
            name: "codebuddy".to_string(),
            base_url: "https://copilot.tencent.com".to_string(),
            request_url: "https://copilot.tencent.com/v2/chat/completions".to_string(),
            api_key: "test-key".to_string(),
            model: "codebuddy-model".to_string(),
            format: "openai".to_string(),
            context_window: 128000,
            max_tokens: None,
            max_images_per_request: None,
            temperature: None,
            top_p: None,
            inline_think_in_text: false,
            custom_headers,
            custom_headers_mode: None,
            skip_ssl_verify: false,
            custom_request_body: None,
            custom_request_body_mode: None,
        }
    }

    fn headers_with(custom_headers: &[(&str, &str)]) -> AIConfig {
        config_with(Some(
            custom_headers
                .iter()
                .map(|(name, value)| ((*name).to_string(), (*value).to_string()))
                .collect(),
        ))
    }

    /// Minimal OpenAI-shaped request body whose first user message is `prompt`.
    fn body_with_prompt(prompt: &str) -> String {
        serde_json::json!({
            "model": "codebuddy-model",
            "messages": [
                { "role": "system", "content": "you are a coding agent" },
                { "role": "user", "content": prompt },
            ],
        })
        .to_string()
    }

    fn build(config: &AIConfig, turn_request_id: Option<&str>) -> Vec<(&'static str, String)> {
        codebuddy_request_headers(
            config,
            "https://copilot.tencent.com/v2/chat/completions",
            turn_request_id,
            "",
        )
    }

    /// Build the header set from a request body, resolving both the turn id and
    /// the turn-stable trace group from that body — the production pairing.
    fn build_from_body(config: &AIConfig, request_body: &str) -> Vec<(&'static str, String)> {
        let turn_id = codebuddy_turn_request_id(request_body);
        codebuddy_request_headers(
            config,
            "https://copilot.tencent.com/v2/chat/completions",
            Some(&turn_id),
            request_body,
        )
    }

    fn find<'a>(headers: &'a [(&'static str, String)], name: &str) -> Option<&'a str> {
        headers
            .iter()
            .find(|(header_name, _)| *header_name == name)
            .map(|(_, value)| value.as_str())
    }

    #[test]
    fn disguise_headers_use_defaults_without_custom_headers() {
        let headers = build(&config_with(None), None);
        assert_eq!(find(&headers, "X-Requested-With"), Some("XMLHttpRequest"));
        assert_eq!(find(&headers, "X-Agent-Intent"), Some("craft"));
        assert_eq!(find(&headers, "X-Agent-Purpose"), Some("conversation"));
        assert_eq!(find(&headers, "X-Agent-Type"), Some("main"));
        assert_eq!(find(&headers, "X-IDE-Type"), Some("CLI"));
        assert_eq!(find(&headers, "X-IDE-Name"), Some("CLI"));
        assert_eq!(find(&headers, "X-IDE-Version"), Some("2.151.0"));
        assert_eq!(find(&headers, "X-CodeBuddy-Request"), Some("1"));
        assert_eq!(find(&headers, "X-Private-Data"), Some("true"));
        assert_eq!(find(&headers, "X-Product"), Some("SaaS"));
        assert_eq!(find(&headers, "X-Api-Key"), Some("test-key"));
        assert_eq!(
            find(&headers, "User-Agent"),
            Some("CLI/2.151.0 CodeBuddy/2.151.0")
        );
    }

    #[test]
    fn stainless_headers_match_capture() {
        let headers = build(&config_with(None), None);
        assert_eq!(find(&headers, "X-Stainless-Arch"), Some("x64"));
        assert_eq!(find(&headers, "X-Stainless-Lang"), Some("js"));
        assert_eq!(find(&headers, "X-Stainless-Os"), Some("Windows"));
        assert_eq!(
            find(&headers, "X-Stainless-Package-Version"),
            Some("6.25.0")
        );
        assert_eq!(find(&headers, "X-Stainless-Retry-Count"), Some("0"));
        assert_eq!(find(&headers, "X-Stainless-Runtime"), Some("node"));
        assert_eq!(
            find(&headers, "X-Stainless-Runtime-Version"),
            Some("v25.2.1")
        );
    }

    #[test]
    fn per_request_ids_are_fresh_and_wellformed() {
        let config = config_with(None);
        let first = build(&config, None);
        let second = build(&config, None);
        for headers in [&first, &second] {
            // Per-request id stays 32-hex wellformed.
            for name in ["X-Request-ID", "X-Conversation-Message-ID"] {
                let value = find(headers, name).expect("per-request id must be present");
                assert_eq!(value.len(), 32, "{name} must be 32 hex chars");
                assert!(value.chars().all(|c| c.is_ascii_hexdigit()));
            }
            // Conversation scope ids stay wellformed.
            let conversation_id =
                find(headers, "X-Conversation-Id").expect("X-Conversation-Id must be present");
            assert_eq!(conversation_id.len(), 36, "conversation id is a UUID");
            let conversation_request_id = find(headers, "X-Conversation-Request-ID")
                .expect("X-Conversation-Request-ID must be present");
            assert_eq!(conversation_request_id.len(), 32);
            assert!(conversation_request_id
                .chars()
                .all(|c| c.is_ascii_hexdigit()));
            // Root mirrors the conversation request id.
            assert_eq!(
                find(headers, "X-Root-Request-Id"),
                find(headers, "X-Conversation-Request-ID")
            );
            // Fallback conversation id is a UUIDv7 (48-bit ms timestamp
            // prefix, version nibble `7`).
            assert_eq!(conversation_id.as_bytes()[14], b'7');
            let stamp: u64 = u64::from_str_radix(&conversation_id.replace('-', "")[0..12], 16)
                .expect("timestamp hex");
            let now_ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_millis() as u64;
            assert!(
                now_ms.saturating_sub(stamp) < 60_000,
                "v7 timestamp {stamp} must be near now {now_ms}"
            );
            // Anonymous user id default.
            let user_id = find(headers, "X-User-Id").expect("X-User-Id must be present");
            assert!(user_id.starts_with("anonymous_"));
            assert_eq!(user_id.len(), "anonymous_".len() + 8);
        }
        // X-Request-ID == X-Conversation-Message-ID inside one request
        // (capture fact) and the pair is fresh on every build.
        assert_eq!(
            find(&first, "X-Request-ID"),
            find(&first, "X-Conversation-Message-ID")
        );
        assert_ne!(find(&first, "X-Request-ID"), find(&second, "X-Request-ID"));
        // Pinned per process: conversation id and user id share the same
        // lifetime (cache prefix stability), so repeated builds reuse them.
        assert_eq!(
            find(&first, "X-Conversation-Id"),
            find(&second, "X-Conversation-Id")
        );
        assert_eq!(find(&first, "X-User-Id"), find(&second, "X-User-Id"));
        // Without a turn context the fallback conversation request id is
        // process-pinned too.
        assert_eq!(
            find(&first, "X-Conversation-Request-ID"),
            find(&second, "X-Conversation-Request-ID")
        );
    }

    #[test]
    fn conversation_request_id_is_turn_stable_from_body() {
        let _serial = turn_state_guard();
        // Official semantics: every request of one user turn carries the same
        // X-Conversation-Request-ID; a new turn gets a new value. The turn is
        // judged from the request body alone, so the adapter needs no upstream
        // context contract.
        let config = config_with(None);
        let body = body_with_prompt("first user prompt");
        let turn_id = codebuddy_turn_request_id(&body);
        let first = build(&config, Some(&turn_id));
        let second = build(&config, Some(&turn_id));
        assert_eq!(
            find(&first, "X-Conversation-Request-ID"),
            Some(turn_id.as_str())
        );
        assert_eq!(
            find(&first, "X-Conversation-Request-ID"),
            find(&second, "X-Conversation-Request-ID")
        );
        assert_eq!(
            find(&first, "X-Root-Request-Id"),
            find(&first, "X-Conversation-Request-ID")
        );
        // Per-request pair keeps rotating across turn-stable calls.
        assert_ne!(find(&first, "X-Request-ID"), find(&second, "X-Request-ID"));

        let next_turn_id = codebuddy_turn_request_id(&body_with_prompt("second user prompt"));
        assert_ne!(turn_id, next_turn_id);
        let third = build(&config, Some(&next_turn_id));
        assert_eq!(
            find(&third, "X-Conversation-Request-ID"),
            Some(next_turn_id.as_str())
        );
        assert_ne!(
            find(&first, "X-Conversation-Request-ID"),
            find(&third, "X-Conversation-Request-ID")
        );
    }

    #[test]
    fn turn_request_id_is_stable_across_calls_with_same_first_user_message() {
        let _serial = turn_state_guard();
        // One user turn spans several outgoing requests (tool follow-ups,
        // retries); the first user message repeats verbatim while later
        // messages are appended, so the id must not rotate.
        let body = body_with_prompt("stable prompt");
        let first = codebuddy_turn_request_id(&body);
        for _ in 0..50 {
            assert_eq!(codebuddy_turn_request_id(&body), first);
        }
        // The growing tool transcript of the same turn keeps the same id.
        let grown = serde_json::json!({
            "model": "codebuddy-model",
            "messages": [
                { "role": "system", "content": "you are a coding agent" },
                { "role": "user", "content": "stable prompt" },
                { "role": "assistant", "content": "reading the file" },
                { "role": "tool", "content": "file contents" },
            ],
        })
        .to_string();
        assert_eq!(codebuddy_turn_request_id(&grown), first);
    }

    #[test]
    fn turn_request_id_rotates_when_first_user_message_changes() {
        let _serial = turn_state_guard();
        let first = codebuddy_turn_request_id(&body_with_prompt("prompt alpha"));
        let second = codebuddy_turn_request_id(&body_with_prompt("prompt beta"));
        assert_ne!(first, second);
        // Fresh ids are UUIDv7: version nibble `7`, variant nibble in [89ab].
        for id in [&first, &second] {
            assert_eq!(id.len(), 36, "turn id must be a UUID");
            assert_eq!(id.as_bytes()[14], b'7', "turn id must be version 7");
            assert!(matches!(id.as_bytes()[19], b'8' | b'9' | b'a' | b'b'));
        }
    }

    #[test]
    fn interleaved_conversations_keep_independent_turn_ids() {
        let _serial = turn_state_guard();
        // Two conversations share this process-wide singleton and interleave
        // their requests (observed live: two threads alternating). Each turn
        // must keep its own id across the interleaving; a single shared "last
        // turn" slot would evict the other conversation on every step and mint
        // a fresh id per request, splitting the backend aggregation group.
        let body_a = body_with_prompt("conversation A turn");
        let body_b = body_with_prompt("conversation B turn");
        let id_a = codebuddy_turn_request_id(&body_a);
        let id_b = codebuddy_turn_request_id(&body_b);
        assert_ne!(id_a, id_b);
        for _ in 0..50 {
            // A follow-up of A after a B request must still be A's turn id.
            assert_eq!(codebuddy_turn_request_id(&body_a), id_a);
            assert_eq!(codebuddy_turn_request_id(&body_b), id_b);
        }
    }

    #[test]
    fn repeated_first_user_message_reuses_the_turn_id() {
        let _serial = turn_state_guard();
        // A retry / stream re-issue repeats the first user message verbatim and
        // must reuse the id so the backend still aggregates the turn.
        let id = codebuddy_turn_request_id(&body_with_prompt("retry prompt"));
        assert_eq!(
            codebuddy_turn_request_id(&body_with_prompt("retry prompt")),
            id
        );
    }

    #[test]
    fn turn_map_stays_bounded_under_many_distinct_turns() {
        let _serial = turn_state_guard();
        for i in 0..(super::MAX_TRACKED_TURNS * 2) {
            let _ = codebuddy_turn_request_id(&body_with_prompt(&format!("turn {i}")));
        }
        let state = super::turn_state()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        assert!(
            state.turns.len() <= super::MAX_TRACKED_TURNS,
            "tracked turns must stay bounded, got {}",
            state.turns.len()
        );
    }

    #[test]
    fn turn_request_id_falls_back_without_panicking_on_unusable_body() {
        let _serial = turn_state_guard();
        // Unparsable or turn-less bodies cannot define a turn: the fallback is
        // the process-pinned anonymous conversation request id, never a panic.
        let fallback = build(&config_with(None), None);
        let expected = find(&fallback, "X-Conversation-Request-ID")
            .expect("fallback id present")
            .to_string();
        for body in [
            "",
            "not json",
            "[]",
            "{}",
            r#"{"messages": []}"#,
            r#"{"messages": [{"role": "assistant", "content": "hi"}]}"#,
        ] {
            assert!(first_user_message_fingerprint(body).is_none());
            assert_eq!(codebuddy_turn_request_id(body), expected);
        }
    }

    #[test]
    fn fingerprint_reads_first_user_message_only() {
        let body = body_with_prompt("hello");
        let fingerprint = first_user_message_fingerprint(&body).expect("fingerprint present");
        assert!(fingerprint.contains("hello"));
        // The system message is never the turn anchor.
        assert!(!fingerprint.contains("coding agent"));
    }

    #[test]
    fn conversation_id_prefers_anonymous_and_custom_channels() {
        // X-Conversation-Id has no turn-scoped source: custom headers first,
        // then the process-pinned anonymous value.
        let custom = headers_with(&[("X-Conversation-Id", "00000000-0000-7000-8000-000000000000")]);
        let headers = build(&custom, None);
        assert_eq!(
            find(&headers, "X-Conversation-Id"),
            Some("00000000-0000-7000-8000-000000000000")
        );
        let headers = build(&custom, Some("turn-value-1"));
        assert_eq!(
            find(&headers, "X-Conversation-Id"),
            Some("00000000-0000-7000-8000-000000000000")
        );
        assert_eq!(
            find(&headers, "X-Conversation-Request-ID"),
            Some("turn-value-1")
        );
    }

    #[test]
    fn trace_group_is_turn_stable_while_span_stays_per_request() {
        let _serial = turn_state_guard();
        // Capture truth (`captured-multi.jsonl`, three requests of one turn):
        // x-b3-traceid / x-b3-parentspanid / x-trace-id are constant within the
        // turn, while x-b3-spanid changes on every request.
        let config = config_with(None);
        let body = body_with_prompt("trace turn prompt");
        let first = build_from_body(&config, &body);
        let second = build_from_body(&config, &body);

        let trace_id = find(&first, "X-B3-TraceId").expect("trace id present");
        assert_eq!(find(&second, "X-B3-TraceId"), Some(trace_id));
        assert_eq!(find(&first, "X-Trace-Id"), Some(trace_id));
        assert_eq!(find(&second, "X-Trace-Id"), Some(trace_id));

        let parent_span_id = find(&first, "X-B3-ParentSpanId").expect("parent span id present");
        assert_eq!(
            find(&second, "X-B3-ParentSpanId"),
            Some(parent_span_id),
            "the parent span id is constant within one turn"
        );

        let span_id = find(&first, "X-B3-SpanId").expect("span id present");
        assert_eq!(span_id.len(), 16);
        assert_ne!(
            find(&second, "X-B3-SpanId"),
            Some(span_id),
            "the span id identifies the individual request"
        );

        // The composite headers are assembled from the very same values.
        for headers in [&first, &second] {
            let trace_id = find(headers, "X-B3-TraceId").expect("trace id present");
            let span_id = find(headers, "X-B3-SpanId").expect("span id present");
            let parent_span_id =
                find(headers, "X-B3-ParentSpanId").expect("parent span id present");
            assert_eq!(
                find(headers, "traceparent").expect("traceparent present"),
                format!("00-{trace_id}-{span_id}-01")
            );
            assert_eq!(
                find(headers, "b3").expect("b3 present"),
                format!("{trace_id}-{span_id}-1-{parent_span_id}")
            );
        }
    }

    #[test]
    fn trace_group_rotates_with_the_turn_and_survives_the_growing_transcript() {
        let _serial = turn_state_guard();
        let config = config_with(None);
        let first_turn = build_from_body(&config, &body_with_prompt("trace prompt alpha"));
        let second_turn = build_from_body(&config, &body_with_prompt("trace prompt beta"));
        assert_ne!(
            find(&first_turn, "X-B3-TraceId"),
            find(&second_turn, "X-B3-TraceId"),
            "a new user turn gets a new trace id"
        );
        assert_ne!(
            find(&first_turn, "X-B3-ParentSpanId"),
            find(&second_turn, "X-B3-ParentSpanId"),
            "a new user turn gets a new parent span id"
        );

        // Tool follow-ups of the same turn repeat the first user message while
        // later messages are appended: the trace group must not rotate.
        let grown = serde_json::json!({
            "model": "codebuddy-model",
            "messages": [
                { "role": "system", "content": "you are a coding agent" },
                { "role": "user", "content": "trace prompt alpha" },
                { "role": "assistant", "content": "reading the file" },
                { "role": "tool", "content": "file contents" },
            ],
        })
        .to_string();
        let follow_up = build_from_body(&config, &grown);
        assert_eq!(
            find(&follow_up, "X-B3-TraceId"),
            find(&first_turn, "X-B3-TraceId")
        );
        assert_eq!(
            find(&follow_up, "X-B3-ParentSpanId"),
            find(&first_turn, "X-B3-ParentSpanId")
        );
    }

    #[test]
    fn trace_group_shares_the_turn_with_the_conversation_request_id() {
        let _serial = turn_state_guard();
        // The trace group and the request-group id are bound to one turn
        // boundary, so both must be stable over exactly the same calls.
        let config = config_with(None);
        let body = body_with_prompt("shared turn boundary");
        let first = build_from_body(&config, &body);
        let second = build_from_body(&config, &body);
        assert_eq!(
            find(&first, "X-Conversation-Request-ID"),
            find(&second, "X-Conversation-Request-ID")
        );
        assert_eq!(find(&first, "X-B3-TraceId"), find(&second, "X-B3-TraceId"));
    }

    #[test]
    fn trace_group_falls_back_to_process_pinned_values_without_a_turn() {
        let _serial = turn_state_guard();
        // Bodies that cannot define a turn repeat the process-pinned trace
        // group instead of minting a fresh one per request.
        let config = config_with(None);
        let first = build_from_body(&config, "not json");
        let second = build_from_body(&config, "");
        let third = build_from_body(&config, r#"{"messages": []}"#);
        for name in ["X-B3-TraceId", "X-B3-ParentSpanId", "X-Trace-Id"] {
            assert_eq!(find(&first, name), find(&second, name), "{name} pinned");
            assert_eq!(find(&first, name), find(&third, name), "{name} pinned");
        }
        // Both fallbacks agree with the anonymous conversation request id
        // lifetime, which is the same process-pinned identity.
        let first_id = find(&first, "X-Conversation-Request-ID").expect("request id present");
        let third_id = find(&third, "X-Conversation-Request-ID").expect("request id present");
        assert_eq!(first_id, third_id);
        // The span id still rotates, so the requests stay distinguishable.
        assert_ne!(find(&first, "X-B3-SpanId"), find(&second, "X-B3-SpanId"));
    }

    #[test]
    fn custom_headers_override_the_trace_group() {
        // custom_headers keep their override-everything semantics, and the
        // composite headers follow the overridden values.
        let config = headers_with(&[
            ("X-B3-TraceId", "11111111111111111111111111111111"),
            ("X-B3-ParentSpanId", "2222222222222222"),
        ]);
        let headers = build_from_body(&config, &body_with_prompt("override turn"));
        assert_eq!(
            find(&headers, "X-B3-TraceId"),
            Some("11111111111111111111111111111111")
        );
        assert_eq!(
            find(&headers, "X-Trace-Id"),
            Some("11111111111111111111111111111111")
        );
        assert_eq!(
            find(&headers, "X-B3-ParentSpanId"),
            Some("2222222222222222")
        );
        let span_id = find(&headers, "X-B3-SpanId").expect("span id present");
        assert_eq!(
            find(&headers, "traceparent"),
            Some(format!("00-11111111111111111111111111111111-{span_id}-01").as_str())
        );
        assert_eq!(
            find(&headers, "b3"),
            Some(format!("11111111111111111111111111111111-{span_id}-1-2222222222222222").as_str())
        );
    }

    #[test]
    fn empty_turn_values_fall_back_to_anonymous_and_custom() {
        let config = config_with(None);
        // An empty turn id must not reach the wire; fallback path applies
        // (process-pinned anonymous values).
        let first = build(&config, Some(""));
        let second = build(&config, None);
        assert_eq!(
            find(&first, "X-Conversation-Request-ID"),
            find(&second, "X-Conversation-Request-ID")
        );
        // A custom header still wins over the anonymous fallback when the
        // turn has no value.
        let custom = headers_with(&[(
            "X-Conversation-Request-ID",
            "cccccccccccccccccccccccccccccccc",
        )]);
        let headers = build(&custom, None);
        assert_eq!(
            find(&headers, "X-Conversation-Request-ID"),
            Some("cccccccccccccccccccccccccccccccc")
        );
    }

    #[test]
    fn custom_headers_override_turn_values() {
        // Precedence: custom_headers > turn id > anonymous fallback.
        let config = headers_with(&[(
            "X-Conversation-Request-ID",
            "dddddddddddddddddddddddddddddddd",
        )]);
        let headers = build(&config, Some("turn-from-body"));
        assert_eq!(
            find(&headers, "X-Conversation-Request-ID"),
            Some("dddddddddddddddddddddddddddddddd")
        );
        assert_eq!(
            find(&headers, "X-Root-Request-Id"),
            Some("dddddddddddddddddddddddddddddddd")
        );
    }

    #[test]
    fn anonymous_identity_is_pinned_per_process() {
        let config = config_with(None);
        let pinned_conversation_id = {
            let headers = build(&config, None);
            find(&headers, "X-Conversation-Id")
                .expect("conversation id present")
                .to_string()
        };
        let pinned_conversation_request_id = {
            let headers = build(&config, None);
            find(&headers, "X-Conversation-Request-ID")
                .expect("conversation request id present")
                .to_string()
        };
        let pinned_user_id = {
            let headers = build(&config, None);
            find(&headers, "X-User-Id")
                .expect("user id present")
                .to_string()
        };
        for _ in 0..50 {
            let headers = build(&config, None);
            assert_eq!(
                find(&headers, "X-Conversation-Id"),
                Some(pinned_conversation_id.as_str())
            );
            assert_eq!(
                find(&headers, "X-Conversation-Request-ID"),
                Some(pinned_conversation_request_id.as_str())
            );
            assert_eq!(find(&headers, "X-User-Id"), Some(pinned_user_id.as_str()));
        }
    }

    #[test]
    fn trace_headers_match_capture_shape() {
        let headers = build(&config_with(None), None);
        let traceparent = find(&headers, "traceparent").expect("traceparent present");
        let b3 = find(&headers, "b3").expect("b3 present");
        let trace_id = find(&headers, "X-B3-TraceId").expect("trace id present");
        let span_id = find(&headers, "X-B3-SpanId").expect("span id present");
        let parent_span_id = find(&headers, "X-B3-ParentSpanId").expect("parent span id present");

        assert_eq!(
            traceparent.splitn(4, '-').collect::<Vec<_>>(),
            vec!["00", trace_id, span_id, "01"]
        );
        assert_eq!(
            b3.splitn(4, '-').collect::<Vec<_>>(),
            vec![trace_id, span_id, "1", parent_span_id]
        );
        assert_eq!(find(&headers, "X-Trace-Id"), Some(trace_id));
        assert_eq!(find(&headers, "X-B3-Sampled"), Some("1"));
        assert_eq!(trace_id.len(), 32);
        assert_eq!(span_id.len(), 16);
        assert_eq!(parent_span_id.len(), 16);
    }

    #[test]
    fn host_gate_matches_the_captured_url_and_rejects_hostile_shapes() {
        // The gate must accept the real host and reject every URL that only
        // embeds it, which is the class a substring comparison lets through.
        assert!(super::is_codebuddy_url(
            "https://copilot.tencent.com/v2/chat/completions"
        ));
        // Substring-in-path / -query / -userinfo / suffix-domain shapes.
        for url in [
            "https://evil.example.com/v1/chat/completions?x=copilot.tencent.com",
            "https://evil.example.com/copilot.tencent.com/v1/chat/completions",
            "https://copilot.tencent.com@evil.example.com/v1/chat/completions",
            "https://copilot.tencent.com.evil.example.com/v1/chat/completions",
        ] {
            assert!(!super::is_codebuddy_url(url), "must reject: {url}");
        }
    }

    #[test]
    fn host_gate_accepts_host_forms_the_url_parser_would() {
        // Same host, shapes the URL parser normalizes to `host_str()` equal to
        // the constant: explicit port, root dot, whitespace, and no path.
        for url in [
            "https://copilot.tencent.com",
            "https://copilot.tencent.com/",
            "https://copilot.tencent.com:443/v2/chat/completions",
            "https://copilot.tencent.com./v2/chat/completions",
            "  https://copilot.tencent.com/v2/chat/completions  ",
        ] {
            assert!(super::is_codebuddy_url(url), "must accept: {url}");
        }
    }

    #[test]
    fn custom_headers_override_ide_version_and_user_agent() {
        let config = headers_with(&[
            ("X-IDE-Version", "9.9.9"),
            ("User-Agent", "CLI/9.9.9 CodeBuddy/9.9.9"),
            ("X-IDE-Name", "CodeBuddy-Test"),
            ("X-User-Id", "u-1"),
            ("X-Conversation-Id", "00000000-0000-7000-8000-000000000000"),
            (
                "X-Conversation-Request-ID",
                "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            ),
        ]);
        let headers = build(&config, None);
        assert_eq!(find(&headers, "X-IDE-Version"), Some("9.9.9"));
        assert_eq!(
            find(&headers, "User-Agent"),
            Some("CLI/9.9.9 CodeBuddy/9.9.9")
        );
        assert_eq!(find(&headers, "X-IDE-Name"), Some("CodeBuddy-Test"));
        assert_eq!(find(&headers, "X-User-Id"), Some("u-1"));
        assert_eq!(
            find(&headers, "X-Conversation-Id"),
            Some("00000000-0000-7000-8000-000000000000")
        );
        assert_eq!(
            find(&headers, "X-Conversation-Request-ID"),
            Some("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")
        );
        assert_eq!(
            find(&headers, "X-Root-Request-Id"),
            Some("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")
        );
        // Fixed values are NOT overridable via custom_headers.
        assert_eq!(find(&headers, "X-Agent-Intent"), Some("craft"));
        assert_eq!(find(&headers, "X-IDE-Type"), Some("CLI"));
        assert_eq!(find(&headers, "X-Agent-Type"), Some("main"));
        assert_eq!(find(&headers, "X-Product"), Some("SaaS"));
        // Fixed disguise values are NOT overridable via custom_headers: the
        // user-supplied X-Private-Data below is ignored, the CLI default wins.
        assert_eq!(find(&headers, "X-Private-Data"), Some("true"));
        assert!(find(&headers, "X-Domain").is_none());
    }

    #[test]
    fn identity_headers_follow_custom_headers_presence() {
        let config = headers_with(&[
            ("X-Userinfo", "YWxpY2U="),
            ("X-Enterprise-Id", "ent-1"),
            ("X-Tenant-Id", "tenant-1"),
            ("X-Department-Info", "dept-1"),
            ("X-Id-Source", "src-1"),
            ("X-Auth-Method", "pat"),
        ]);
        let headers = build(&config, None);
        assert_eq!(find(&headers, "X-Userinfo"), Some("YWxpY2U="));
        assert_eq!(find(&headers, "X-Enterprise-Id"), Some("ent-1"));
        assert_eq!(find(&headers, "X-Tenant-Id"), Some("tenant-1"));
        assert_eq!(find(&headers, "X-Department-Info"), Some("dept-1"));
        assert_eq!(find(&headers, "X-Id-Source"), Some("src-1"));
        assert_eq!(find(&headers, "X-Auth-Method"), Some("pat"));

        // Absent identity headers stay absent (never defaulted).
        let headers = build(&config_with(None), None);
        assert!(find(&headers, "X-Userinfo").is_none());
        assert!(find(&headers, "X-Enterprise-Id").is_none());
        assert!(find(&headers, "X-Tenant-Id").is_none());
        assert!(find(&headers, "X-Department-Info").is_none());
        assert!(find(&headers, "X-Id-Source").is_none());
        assert!(find(&headers, "X-Auth-Method").is_none());
    }

    #[test]
    fn capture_absent_headers_stay_absent() {
        // X-Domain is absent from both captures (CLI 2.147.0 single-request and
        // 2.148.0 multi-request); it is never emitted, even if configured.
        // X-Private-Data is the opposite case: absent in 2.147.0 but present as
        // `true` on every 2.148.0 request, so it is emitted as a fixed value and
        // ignores user configuration.
        for config in [
            config_with(None),
            headers_with(&[("X-Domain", "evil.example.com")]),
        ] {
            let headers = build(&config, None);
            assert!(find(&headers, "X-Domain").is_none());
            assert_eq!(find(&headers, "X-Private-Data"), Some("true"));
        }
    }

    #[test]
    fn disguise_headers_cover_full_cli_set_and_ordering_is_stable() {
        let config = headers_with(&[
            ("X-Userinfo", "YWxpY2U="),
            ("X-Enterprise-Id", "ent-1"),
            ("X-Tenant-Id", "tenant-1"),
            ("X-Department-Info", "dept-1"),
            ("X-Id-Source", "src-1"),
            ("X-Auth-Method", "pat"),
        ]);
        let headers = build(&config, None);
        let names: Vec<&str> = headers.iter().map(|(name, _)| *name).collect();
        assert_eq!(
            names,
            vec![
                "X-Requested-With",
                "X-Stainless-Arch",
                "X-Stainless-Lang",
                "X-Stainless-Os",
                "X-Stainless-Package-Version",
                "X-Stainless-Retry-Count",
                "X-Stainless-Runtime",
                "X-Stainless-Runtime-Version",
                "X-Conversation-Id",
                "X-Agent-Intent",
                "X-Agent-Purpose",
                "X-IDE-Type",
                "X-IDE-Name",
                "X-IDE-Version",
                "X-CodeBuddy-Request",
                "X-Private-Data",
                "X-Api-Key",
                "X-Request-ID",
                "X-Conversation-Message-ID",
                "X-Root-Request-Id",
                "X-Conversation-Request-ID",
                "X-Agent-Type",
                "traceparent",
                "b3",
                "X-B3-TraceId",
                "X-B3-ParentSpanId",
                "X-B3-SpanId",
                "X-B3-Sampled",
                "X-Trace-Id",
                "X-User-Id",
                "X-Product",
                "User-Agent",
                "X-Userinfo",
                "X-Enterprise-Id",
                "X-Tenant-Id",
                "X-Department-Info",
                "X-Id-Source",
                "X-Auth-Method",
            ]
        );
        assert_eq!(names.len(), 38);
    }

    #[test]
    fn anonymous_wire_matches_capture_header_set() {
        // Anonymous channel: identity headers absent, everything else present.
        let headers = build(&config_with(None), None);
        let names: std::collections::HashSet<&str> =
            headers.iter().map(|(name, _)| *name).collect();
        let captured = [
            "X-Requested-With",
            "X-Stainless-Arch",
            "X-Stainless-Lang",
            "X-Stainless-Os",
            "X-Stainless-Package-Version",
            "X-Stainless-Retry-Count",
            "X-Stainless-Runtime",
            "X-Stainless-Runtime-Version",
            "X-Conversation-Id",
            "X-Agent-Intent",
            "X-Agent-Purpose",
            "X-IDE-Type",
            "X-IDE-Name",
            "X-IDE-Version",
            "X-CodeBuddy-Request",
            "X-Private-Data",
            "X-Api-Key",
            "X-Request-ID",
            "X-Conversation-Message-ID",
            "X-Root-Request-Id",
            "X-Conversation-Request-ID",
            "X-Agent-Type",
            "traceparent",
            "b3",
            "X-B3-TraceId",
            "X-B3-ParentSpanId",
            "X-B3-SpanId",
            "X-B3-Sampled",
            "X-Trace-Id",
            "X-User-Id",
            "X-Product",
            "User-Agent",
        ];
        for name in captured {
            assert!(names.contains(name), "missing wire header: {name}");
        }
        for name in [
            "X-Userinfo",
            "X-Enterprise-Id",
            "X-Tenant-Id",
            "X-Department-Info",
            "X-Id-Source",
            "X-Auth-Method",
        ] {
            assert!(
                !names.contains(name),
                "identity header must stay absent: {name}"
            );
        }
        assert_eq!(names.len(), captured.len());
    }

    #[test]
    fn anonymous_wire_ordering_matches_capture() {
        // Anonymous shape (no identity headers): full set in wire order.
        let headers = build(&config_with(None), None);
        let names: Vec<&str> = headers.iter().map(|(name, _)| *name).collect();
        assert_eq!(
            names,
            vec![
                "X-Requested-With",
                "X-Stainless-Arch",
                "X-Stainless-Lang",
                "X-Stainless-Os",
                "X-Stainless-Package-Version",
                "X-Stainless-Retry-Count",
                "X-Stainless-Runtime",
                "X-Stainless-Runtime-Version",
                "X-Conversation-Id",
                "X-Agent-Intent",
                "X-Agent-Purpose",
                "X-IDE-Type",
                "X-IDE-Name",
                "X-IDE-Version",
                "X-CodeBuddy-Request",
                "X-Private-Data",
                "X-Api-Key",
                "X-Request-ID",
                "X-Conversation-Message-ID",
                "X-Root-Request-Id",
                "X-Conversation-Request-ID",
                "X-Agent-Type",
                "traceparent",
                "b3",
                "X-B3-TraceId",
                "X-B3-ParentSpanId",
                "X-B3-SpanId",
                "X-B3-Sampled",
                "X-Trace-Id",
                "X-User-Id",
                "X-Product",
                "User-Agent",
            ]
        );
        assert_eq!(names.len(), 32);
    }

    #[test]
    fn per_request_ids_stay_fresh_across_thousands_of_calls() {
        // Deterministic stress: the wire-attack class is two ids within one
        // request colliding; iterate far beyond the LCG low-bit period.
        // Conversation/user ids are pinned per process (see
        // anonymous_identity_is_pinned_per_process); freshness applies to
        // the per-request id (X-Request-ID, mirrored by
        // X-Conversation-Message-ID).
        let config = config_with(None);
        let mut seen_request_ids = std::collections::HashSet::new();
        for _ in 0..2000 {
            let headers = build(&config, None);
            let request_id = find(&headers, "X-Request-ID").expect("X-Request-ID present");
            let message_id =
                find(&headers, "X-Conversation-Message-ID").expect("message id present");
            // Capture fact: both ids inside ONE request carry the same value.
            assert_eq!(request_id, message_id, "in-request ids must mirror");
            assert!(seen_request_ids.insert(request_id.to_string()));
        }
        assert_eq!(seen_request_ids.len(), 2000);
    }

    #[test]
    fn random_hex_has_no_period16_self_repeat() {
        // Structural defect regression: the LCG low nibble has period 16, so
        // the old direct `state & 0x0F` sampling made h[:16] == h[16:] always
        // hold for len 32. The shift-per-position sampler breaks that class.
        let half = super::random_hex(16);
        let full = super::random_hex(32);
        assert_eq!(full.len(), 32);
        assert_ne!(&full[0..16], &full[16..], "period-16 self repeat");
        assert!(half.chars().all(|c| c.is_ascii_hexdigit()));
        let samples: Vec<String> = (0..200).map(|_| super::random_hex(32)).collect();
        let unique: std::collections::HashSet<&String> = samples.iter().collect();
        assert_eq!(unique.len(), samples.len());
    }

    #[test]
    fn turn_request_id_is_passed_through_untouched() {
        // The id already carries the time semantics (UUIDv7); the adapter
        // passes it through verbatim instead of reshaping it.
        let config = config_with(None);
        let v7 = "0198f0a2-7d3e-7abc-9def-0123456789ab";
        let headers = build(&config, Some(v7));
        assert_eq!(find(&headers, "X-Conversation-Request-ID"), Some(v7));
        // A second call within the same turn keeps the identical value.
        let headers_again = build(&config, Some(v7));
        assert_eq!(find(&headers_again, "X-Conversation-Request-ID"), Some(v7));
    }

    #[test]
    fn clears_empty_finish_reason() {
        let response = normalize_response(UnifiedResponse {
            finish_reason: Some("".to_string()),
            tool_call_completion: Some(ToolCallCompletion::Unknown),
            ..Default::default()
        });
        assert!(response.finish_reason.is_none());
        assert!(response.tool_call_completion.is_none());
    }

    #[test]
    fn clears_whitespace_finish_reason() {
        let response = normalize_response(UnifiedResponse {
            finish_reason: Some("   ".to_string()),
            tool_call_completion: Some(ToolCallCompletion::Unknown),
            ..Default::default()
        });
        assert!(response.finish_reason.is_none());
        assert!(response.tool_call_completion.is_none());
    }

    #[test]
    fn preserves_legal_finish_reasons() {
        for reason in [
            "stop",
            "length",
            "tool_calls",
            "content_filter",
            "function_call",
        ] {
            let response = normalize_response(UnifiedResponse {
                finish_reason: Some(reason.to_string()),
                tool_call_completion: Some(ToolCallCompletion::NormalToolUse),
                ..Default::default()
            });
            assert_eq!(response.finish_reason.as_deref(), Some(reason));
            assert_eq!(
                response.tool_call_completion,
                Some(ToolCallCompletion::NormalToolUse)
            );
        }
    }

    #[test]
    fn preserves_unknown_non_empty_finish_reasons() {
        // Non-empty provider-specific reasons are NOT our business: upstream
        // maps them deliberately. Only empty/whitespace is a deviation.
        let response = normalize_response(UnifiedResponse {
            finish_reason: Some("eos".to_string()),
            tool_call_completion: Some(ToolCallCompletion::Unknown),
            ..Default::default()
        });
        assert_eq!(response.finish_reason.as_deref(), Some("eos"));
        assert_eq!(
            response.tool_call_completion,
            Some(ToolCallCompletion::Unknown)
        );
    }

    #[test]
    fn passthrough_when_no_finish_reason() {
        let response = normalize_response(UnifiedResponse {
            text: Some("hello".to_string()),
            ..Default::default()
        });
        assert_eq!(response.text.as_deref(), Some("hello"));
        assert!(response.finish_reason.is_none());
    }

    #[test]
    fn idempotent_under_repeat_application() {
        let once = normalize_response(UnifiedResponse {
            finish_reason: Some("".to_string()),
            tool_call_completion: Some(ToolCallCompletion::Unknown),
            ..Default::default()
        });
        let twice = normalize_response(once.clone());
        assert_eq!(once.finish_reason, twice.finish_reason);
        assert_eq!(once.tool_call_completion, twice.tool_call_completion);
    }
}
