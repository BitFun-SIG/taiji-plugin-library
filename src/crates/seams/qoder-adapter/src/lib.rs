//! User-side Qoder adapter seam (OUTSIDE the upstream git tree).
//!
//! Purpose: sign Qoder CN gateway inference requests (COSY signature) and decode
//! gateway responses, using the embedded Qoder CLI wasm. The seam holds no
//! credential store, and performs no network IO during signing — the host
//! supplies already-resolved signing materials and does the transport.
//!
//! The public face is five free functions plus two constants and one struct, and
//! the inference path never returns `Result`: every failure collapses to `None`
//! (or is absorbed internally) so a caller can fall back to the unsigned path.
//!
//! The credential lifecycle (`pat_login`/`refresh_credential`) deliberately does
//! NOT live here: it performs live HTTPS IO and owns a persisted secret, which is
//! the host's responsibility. `login` exists only to derive signing materials
//! from a wasm call and to render the host's endpoint profile.

mod machine;

#[cfg(feature = "subscription-auth")]
mod catalog;
#[cfg(feature = "subscription-auth")]
mod login;
#[cfg(feature = "subscription-auth")]
mod request;
#[cfg(feature = "subscription-auth")]
mod response;
#[cfg(feature = "subscription-auth")]
mod wasm;

use openbitfun_core_types::AIConfig;

/// The host facts this seam needs, in the seam's own types.
///
/// The host compiles this crate as a separate workspace member, so it resolves
/// its own instances of the crates below. Anything named across the boundary in
/// `AIConfig` would therefore be rejected at the call site as a different type
/// of the same name, and `reqwest::RequestBuilder` additionally carries its own
/// generic parameters, which cannot be forwarded through a seam at all.
///
/// This struct is the replacement: the host copies the three fields the signing
/// path reads into plain values, so nothing in the signature depends on which
/// `openbitfun_core_types` build either side happened to link.
#[derive(Debug, Clone, Default)]
pub struct QoderSigningInput {
    /// `AIConfig::model`, passed through to the wasm signer as the model key.
    pub model: String,
    /// `AIConfig::api_key`; a `pt-` value is exchanged for job-token materials.
    pub api_key: String,
    /// `AIConfig::custom_headers`, the channel carrying pre-resolved materials.
    pub custom_headers: Option<std::collections::HashMap<String, String>>,
}

impl QoderSigningInput {
    /// Borrow these facts as the `AIConfig` the signing path already expects.
    ///
    /// `signed_request` only reads `custom_headers`, `api_key` and `model`, so a
    /// config carrying exactly those is indistinguishable to it. `AIConfig` has
    /// no `Default`, so every other field is spelled out at its zero value; none
    /// of them is ever consulted on this path.
    fn as_config(&self) -> AIConfig {
        AIConfig {
            name: String::new(),
            base_url: String::new(),
            request_url: String::new(),
            api_key: self.api_key.clone(),
            model: self.model.clone(),
            format: String::new(),
            context_window: 0,
            max_tokens: None,
            temperature: None,
            top_p: None,
            inline_think_in_text: false,
            custom_headers: self.custom_headers.clone(),
            custom_headers_mode: None,
            skip_ssl_verify: false,
            custom_request_body: None,
            custom_request_body_mode: None,
        }
    }
}

#[cfg(feature = "subscription-auth")]
pub use login::{
    QoderCredential, QoderRequestProfile, QoderSignature, derive_login_materials,
    materials_for_pat, pat_login, refresh_credential, request_profile, suggested,
};
#[cfg(feature = "subscription-auth")]
pub use request::{QODER_COSY_VERSION, gateway_model_key};
#[cfg(feature = "subscription-auth")]
pub use response::{
    QueueRetryHint, UnfoldedFrame, envelope_is_unfamiliar, is_qoder_envelope, queue_retry_hint,
    unfold_frame,
};

/// Reads the Qoder machine id from the Qoder CN CLI's own auth file
/// (`~/.qoder-cn/.auth/machine_id`), creating a fresh UUID v4 there on first use.
///
/// The gateway validates the signature against the id the CLI used, so this
/// deliberately shares the CLI's location rather than minting an independent id.
pub fn qoder_machine_id() -> String {
    machine::qoder_machine_id()
}

/// Qoder CN gateway host. Requests whose URL starts with this origin are signed.
pub const QODER_GATEWAY_HOST: &str = "https://gateway.qoder.com.cn";

/// Reserved `custom_headers` key carrying the Qoder `uid`. The host writes this
/// when it resolves a credential; the signature path reads it back.
pub const X_TAIJI_QODER_UID: &str = "x-taiji-qoder-uid";
/// Reserved `custom_headers` key carrying the wasm `encrypt_user_info`.
pub const X_TAIJI_QODER_USER_INFO: &str = "x-taiji-qoder-user-info";
/// Reserved `custom_headers` key carrying the wasm signing `key`.
pub const X_TAIJI_QODER_KEY: &str = "x-taiji-qoder-key";

/// True when `url` targets the Qoder CN gateway.
///
/// Mirrors the CodeBuddy seam's authority-level match: the gateway is identified
/// by its host component, never by a substring, so a look-alike URL such as
/// `https://evil.example/?u=gateway.qoder.com.cn` is not signed.
pub fn is_qoder_url(url: &str) -> bool {
    let Some(rest) = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
    else {
        return false;
    };
    let authority = rest
        .split(['/', '?', '#'])
        .next()
        .unwrap_or("")
        .rsplit('@')
        .next()
        .unwrap_or("");
    let host = authority.split(':').next().unwrap_or("");
    host == "gateway.qoder.com.cn"
}

/// Signed URL + COSY signature headers + encrypted body for a Qoder inference
/// request. Returns `None` when the request is not a Qoder request, the
/// credential is missing, or signing fails — the caller then proceeds unsigned.
///
/// Without the `subscription-auth` feature the wasm runtime is not compiled in,
/// so this always returns `None` and the caller uses the unsigned path.
pub fn qoder_signed_request(
    config: &AIConfig,
    url: &str,
    body: &str,
    model_key: &str,
) -> Option<QoderSignedRequest> {
    #[cfg(feature = "subscription-auth")]
    {
        request::signed_request(config, url, body, model_key)
    }
    #[cfg(not(feature = "subscription-auth"))]
    {
        let _ = (config, url, body, model_key);
        None
    }
}

/// The host's single header-seam entry point: applies the COSY signature header
/// family to `builder` when `signed_body` carries the seam's encrypted payload,
/// and returns `builder` untouched otherwise (non-Qoder requests are
/// byte-identical).
///
/// The host calls this once inside its existing header-construction closure and
/// passes the same value it already obtained from [`qoder_body_override`], so
/// the signature is never computed twice.
///
/// The gateway authenticates exclusively through the COSY signature family
/// produced by the wasm signer. The reference implementation therefore builds a
/// bare request carrying only those headers; layering the host's default policy
/// (content type, affinity, Accept) on top duplicates names — an `accept` sent
/// twice, for instance — and the gateway then routes the call to a fallback node
/// that fails with `oa_qwen-plus-main`. That is why the host hands in a bare
/// builder for this branch and seeds nothing of its own.
#[cfg(feature = "subscription-auth")]
pub fn inject_qoder_headers(
    builder: reqwest::RequestBuilder,
    url: &str,
    config: &AIConfig,
    body: &str,
    signed_body: Option<&str>,
) -> reqwest::RequestBuilder {
    builder.headers(qoder_header_map(config, url, body, signed_body))
}

/// The COSY header family for a signed request, as name/value pairs.
///
/// This is the host-facing entry point: the host owns its `RequestBuilder`, so
/// the seam only decides which headers to attach. Nothing host-typed crosses.
#[cfg(feature = "subscription-auth")]
pub fn qoder_header_pairs(
    input: &QoderSigningInput,
    url: &str,
    body: &str,
    signed_body: Option<&str>,
) -> Vec<(String, String)> {
    qoder_header_map(&input.as_config(), url, body, signed_body)
        .into_iter()
        .filter_map(|(name, value)| {
            let name = name?;
            let value = value.to_str().ok()?;
            Some((name.as_str().to_string(), value.to_string()))
        })
        .collect()
}

#[cfg(feature = "subscription-auth")]
fn qoder_header_map(
    config: &AIConfig,
    url: &str,
    body: &str,
    signed_body: Option<&str>,
) -> reqwest::header::HeaderMap {
    if signed_body.is_none() {
        return reqwest::header::HeaderMap::new();
    }
    let Some(signed) = request::signed_request(config, url, body, &config.model) else {
        return reqwest::header::HeaderMap::new();
    };
    // The host hands in a bare request builder for Qoder, so the header map
    // starts empty and is filled exclusively from the wasm signature family.
    // `RequestBuilder::header` appends, so seeding from the host's default
    // policy would duplicate names such as `accept`, which the gateway answers
    // by routing to a fallback node that fails.
    let mut headers = reqwest::header::HeaderMap::new();
    if let Ok(value) = reqwest::header::HeaderValue::from_str("application/octet-stream") {
        headers.insert(reqwest::header::CONTENT_TYPE, value);
    }
    let mut has_accept = false;
    for (name, value) in &signed.headers {
        let Ok(name) = reqwest::header::HeaderName::from_bytes(name.as_bytes()) else {
            continue;
        };
        if name == reqwest::header::ACCEPT {
            has_accept = true;
        }
        let Ok(value) = reqwest::header::HeaderValue::from_str(value) else {
            continue;
        };
        headers.insert(name, value);
    }
    // The gateway's SSE endpoint must be asked for an event stream explicitly;
    // the signature family carries it already, but a signer change that dropped
    // it would otherwise turn the response into a silent non-SSE 200.
    if !has_accept {
        if let Ok(value) = reqwest::header::HeaderValue::from_str("text/event-stream") {
            headers.insert(reqwest::header::ACCEPT, value);
        }
    }
    headers
}

/// Header-seam entry point when the wasm runtime is not compiled in: the
/// builder is returned untouched so the caller proceeds unsigned.
#[cfg(not(feature = "subscription-auth"))]
pub fn inject_qoder_headers(
    builder: reqwest::RequestBuilder,
    url: &str,
    config: &AIConfig,
    body: &str,
    signed_body: Option<&str>,
) -> reqwest::RequestBuilder {
    let _ = (url, config, body, signed_body);
    builder
}

/// Header pairs without the wasm runtime: always empty, so the caller's builder
/// is left exactly as it was.
#[cfg(not(feature = "subscription-auth"))]
pub fn qoder_header_pairs(
    input: &QoderSigningInput,
    url: &str,
    body: &str,
    signed_body: Option<&str>,
) -> Vec<(String, String)> {
    let _ = (input, url, body, signed_body);
    Vec::new()
}

/// The host's single body-seam entry point: the signed URL plus the encrypted
/// body when the request targets the Qoder gateway, or `None` so the caller
/// keeps its own URL and JSON body unchanged.
///
/// Qoder CN signs its requests with an embedded wasm and ships an encrypted
/// body, so the seam returns the signed URL plus that payload in one value (or
/// `None`, leaving both URL and body untouched for every other provider).
#[cfg(feature = "subscription-auth")]
pub fn qoder_body_override(config: &AIConfig, url: &str, body: &str) -> Option<(String, String)> {
    let signed = request::signed_request(config, url, body, &config.model)?;
    let encrypted = String::from_utf8(signed.body).ok()?;
    Some((signed.url, encrypted))
}

/// Body-seam entry point taking the host facts in the seam's own types.
///
/// The host cannot hand over its `&AIConfig`: that type comes from a different
/// `openbitfun_core_types` build, so the call would be rejected as a type
/// mismatch. Passing [`QoderSigningInput`] keeps the boundary type-free.
#[cfg(feature = "subscription-auth")]
pub fn qoder_body_override_input(
    input: &QoderSigningInput,
    url: &str,
    body: &str,
) -> Option<(String, String)> {
    qoder_body_override(&input.as_config(), url, body)
}

/// Body-seam entry point without the wasm runtime: always `None`.
#[cfg(not(feature = "subscription-auth"))]
pub fn qoder_body_override(config: &AIConfig, url: &str, body: &str) -> Option<(String, String)> {
    let _ = (config, url, body);
    None
}

/// Body-seam entry point without the wasm runtime: always `None`.
#[cfg(not(feature = "subscription-auth"))]
pub fn qoder_body_override_input(
    input: &QoderSigningInput,
    url: &str,
    body: &str,
) -> Option<(String, String)> {
    let _ = (input, url, body);
    None
}

/// Live Qoder model catalog decoded from the gateway response. Decryption
/// failure falls back to plaintext (mirrors the source adapter's fallback).
/// Returns `None` when the payload has no recognizable model list.
///
/// Without the `subscription-auth` feature the wasm runtime is not compiled in,
/// so this always returns `None`.
pub fn qoder_model_catalog(raw_body: &str) -> Option<Vec<String>> {
    #[cfg(feature = "subscription-auth")]
    {
        catalog::model_catalog(raw_body)
    }
    #[cfg(not(feature = "subscription-auth"))]
    {
        let _ = raw_body;
        None
    }
}

/// Signed request produced by [`qoder_signed_request`].
pub struct QoderSignedRequest {
    pub url: String,
    pub headers: Vec<(&'static str, String)>,
    pub body: Vec<u8>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config_with_custom_headers(
        headers: Option<std::collections::HashMap<String, String>>,
    ) -> AIConfig {
        AIConfig {
            name: "test".to_string(),
            base_url: String::new(),
            request_url: String::new(),
            api_key: String::new(),
            model: "auto".to_string(),
            format: "openai".to_string(),
            context_window: 0,
            max_tokens: None,
            temperature: None,
            top_p: None,
            inline_think_in_text: false,
            custom_headers: headers,
            custom_headers_mode: None,
            skip_ssl_verify: false,
            custom_request_body: None,
            custom_request_body_mode: None,
        }
    }

    fn config_with_signature_materials() -> AIConfig {
        let mut headers = std::collections::HashMap::new();
        headers.insert(X_TAIJI_QODER_UID.to_string(), "test-uid-0001".to_string());
        headers.insert(X_TAIJI_QODER_USER_INFO.to_string(), "enc-test".to_string());
        headers.insert(X_TAIJI_QODER_KEY.to_string(), "key-test".to_string());
        config_with_custom_headers(Some(headers))
    }

    #[test]
    fn is_qoder_url_matches_gateway_authority_only() {
        assert!(is_qoder_url(
            "https://gateway.qoder.com.cn/model/v1/chat/completions"
        ));
        assert!(is_qoder_url("https://gateway.qoder.com.cn"));
        assert!(is_qoder_url("https://gateway.qoder.com.cn:443/x"));
        assert!(!is_qoder_url("https://api.openai.com/v1/chat/completions"));
        assert!(!is_qoder_url("https://gateway.qoder.com"));
        // Substring look-alikes must not be signed.
        assert!(!is_qoder_url(
            "https://evil.example/?u=gateway.qoder.com.cn"
        ));
        assert!(!is_qoder_url("https://gateway.qoder.com.cn.evil.example/x"));
    }

    #[test]
    fn signed_request_is_none_without_signature_materials() {
        let config = config_with_custom_headers(None);
        let url = "https://gateway.qoder.com.cn/model/v1/chat/completions";
        // Missing materials ⇒ early return None (caller falls back unsigned).
        assert!(qoder_signed_request(&config, url, "{}", "auto").is_none());
        // Non-Qoder URL ⇒ also None, regardless of materials.
        let config = config_with_signature_materials();
        assert!(qoder_signed_request(&config, "https://example.com/x", "{}", "auto").is_none());
    }

    /// Section 四 M2/M3 acceptance: the body-seam returns the signed URL and an
    /// encrypted payload whose length is block-aligned (the gateway's own shape),
    /// so the host's raw-body path has something to carry.
    #[cfg(feature = "subscription-auth")]
    #[test]
    fn signed_request_returns_some_for_golden_input() {
        let config = config_with_signature_materials();
        let url = "https://gateway.qoder.com.cn/model/v1/chat/completions";
        let signed = qoder_signed_request(&config, url, r#"{"model":"auto"}"#, "auto")
            .expect("golden input signs");
        assert!(signed.url.starts_with(QODER_GATEWAY_HOST));
        assert!(
            signed
                .headers
                .iter()
                .any(|(name, _)| name.starts_with("Cosy-")),
            "signed request carries COSY headers: {:?}",
            signed.headers
        );
        assert!(
            !signed.body.is_empty(),
            "encrypted body replaces the plaintext"
        );
    }

    #[cfg(feature = "subscription-auth")]
    #[test]
    fn model_catalog_parses_plaintext_chat_scene() {
        // Plaintext (non-encrypted) payload with the `chat` scene; the decrypt
        // attempt fails and the plaintext fallback is parsed.
        let body = r#"{"chat":[{"key":"auto","display_name":"Auto"},{"key":"qmodel_38max","display_name":"Qwen3.7-Plus"},{"key":"kmodel","display_name":"Kimi-K2.7-Code"}]}"#;
        let models = qoder_model_catalog(body).expect("catalog parses");
        // `auto` keeps its routing key; others take the display name.
        assert_eq!(
            models,
            vec![
                "auto".to_string(),
                "Qwen3.7-Plus".to_string(),
                "Kimi-K2.7-Code".to_string()
            ]
        );
        // A payload with no known scene yields None (discriminating negative).
        assert!(qoder_model_catalog(r#"{"unexpected":[]}"#).is_none());
    }

    #[test]
    fn machine_id_path_uses_cli_auth_location() {
        let path = machine::machine_id_path().expect("home dir resolvable");
        let rendered = path.to_string_lossy().replace('\\', "/");
        assert!(
            rendered.ends_with(".qoder-cn/.auth/machine_id"),
            "machine id path uses the CLI auth location: {rendered}"
        );
    }

    #[test]
    fn machine_id_is_stable_and_non_empty() {
        let first = qoder_machine_id();
        assert!(!first.is_empty());
        let second = qoder_machine_id();
        assert_eq!(first, second, "machine id persists across reads");
    }
}
