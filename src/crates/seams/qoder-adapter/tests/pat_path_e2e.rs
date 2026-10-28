//! Proves the *host* path: a bare Qoder PAT in the ordinary API-key field.
//!
//! The sibling `live_e2e` probe pre-populates `custom_headers` with materials it
//! derived itself, so it never exercises `resolve_materials` -> the seam's own
//! PAT -> job-token exchange. The host does exactly the opposite: the user pastes
//! a PAT into the API-key field, `custom_headers` stays empty, and the seam is
//! expected to derive the materials on its own.
//!
//! This probe reproduces the host's shape so a silent fall-back to the unsigned
//! path (which the gateway answers with 503) shows up as a red test.
//!
//! Ignored by default (network + live credential). Run explicitly:
//!
//! ```text
//! $env:BITFUN_QODER_PAT = "<pt-...>"
//! cargo test --features subscription-auth --test pat_path_e2e -- --ignored --nocapture
//! ```

#![cfg(feature = "subscription-auth")]

use openbitfun_core_types::AIConfig;
use taiji_qoder_adapter::qoder_body_override;

/// The Qoder CN production inference endpoint the wasm signs towards.
const MODEL_BASE_URL: &str = "https://gateway.qoder.com.cn";
/// Exactly what the host builds from the user's base URL plus the OpenAI route.
const REQUEST_URL: &str = "https://gateway.qoder.com.cn/chat/completions";

/// The host's config shape for a Qoder account: the PAT lives in `api_key`, and
/// nothing pre-populates `custom_headers`.
fn host_config(pat: &str) -> AIConfig {
    AIConfig {
        name: "qoder".to_string(),
        base_url: MODEL_BASE_URL.to_string(),
        request_url: REQUEST_URL.to_string(),
        api_key: pat.to_string(),
        model: "auto".to_string(),
        format: "openai".to_string(),
        context_window: 0,
        max_tokens: None,
        temperature: None,
        top_p: None,
        inline_think_in_text: false,
        custom_headers: None,
        custom_headers_mode: None,
        skip_ssl_verify: false,
        custom_request_body: None,
        custom_request_body_mode: None,
    }
}

#[test]
#[ignore = "live network + credential; run with --ignored"]
fn bare_pat_in_api_key_field_is_signed_by_the_seam() {
    let pat = std::env::var("BITFUN_QODER_PAT").expect("BITFUN_QODER_PAT must be set");
    assert!(!pat.is_empty(), "BITFUN_QODER_PAT must not be empty");
    assert!(
        pat.starts_with("pt-"),
        "the probe needs a personal access token; got a value without the pt- prefix"
    );

    let body = serde_json::json!({
        "model": "auto",
        "messages": [{ "role": "user", "content": "Reply with exactly: PONG" }],
        "stream": true,
    })
    .to_string();

    // This is the exact call the host makes from its async request builder. It
    // must derive the signing materials from the PAT on its own, from inside a
    // live Tokio runtime.
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("probe runtime builds");

    let outcome = runtime.block_on(async {
        tokio::task::spawn_blocking(move || {
            let config = host_config(&pat);
            qoder_body_override(&config, REQUEST_URL, &body)
        })
        .await
        .expect("probe task joins")
    });

    match outcome {
        Some((signed_url, encrypted)) => {
            println!(
                "[pat-path] signed_url={signed_url} encrypted_len={}",
                encrypted.len()
            );
            assert!(
                signed_url.contains("/algo/api/v2/service/pro/sse/agent_chat_generation"),
                "signed URL must be the gateway's agent_chat_generation endpoint, got {signed_url}"
            );
            assert!(!encrypted.is_empty(), "encrypted body must not be empty");
            assert_eq!(
                encrypted.len() % 16,
                0,
                "encrypted body must be block-aligned, got {}",
                encrypted.len()
            );
        }
        None => panic!(
            "the seam returned None for a bare PAT in api_key: the host would fall back to \
             the unsigned path and the gateway answers 503. This is the defect."
        ),
    }
}
