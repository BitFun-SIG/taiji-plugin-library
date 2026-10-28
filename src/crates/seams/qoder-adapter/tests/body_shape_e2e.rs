//! Determines whether the *body shape* is what the gateway rejects with 400.
//!
//! The host sends its ordinary OpenAI envelope (messages + tools + stream +
//! provider extras). The CLI sends a Qoder-native infer payload instead. Both
//! get encrypted by the same wasm, so the only way to tell whether the gateway
//! accepts the host's shape is to sign each and read the status.
//!
//! Ignored by default (network + live credential). Run explicitly:
//!
//! ```text
//! $env:BITFUN_QODER_PAT = "<pt-...>"
//! cargo test --features subscription-auth --test body_shape_e2e -- --ignored --nocapture
//! ```

#![cfg(feature = "subscription-auth")]

use openbitfun_core_types::AIConfig;
use taiji_qoder_adapter::{inject_qoder_headers, qoder_body_override};

const GATEWAY_URL: &str = "https://gateway.qoder.com.cn";
const REQUEST_URL: &str = "https://gateway.qoder.com.cn/chat/completions";

fn host_config(pat: &str, model: &str, context_window: u32) -> AIConfig {
    AIConfig {
        name: "qoder".to_string(),
        base_url: GATEWAY_URL.to_string(),
        request_url: REQUEST_URL.to_string(),
        api_key: pat.to_string(),
        model: model.to_string(),
        format: "openai".to_string(),
        context_window,
        max_tokens: None,
        max_images_per_request: None,
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
fn which_body_shape_the_gateway_accepts() {
    let pat = std::env::var("BITFUN_QODER_PAT").expect("BITFUN_QODER_PAT must be set");
    assert!(
        pat.starts_with("pt-"),
        "probe needs a personal access token"
    );

    let _ = rustls::crypto::ring::default_provider().install_default();
    let client = reqwest::Client::builder().build().expect("client builds");
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("probe runtime builds");

    // Each case: (label, model_key, body, context_window)
    let tools = serde_json::json!([{
        "type": "function",
        "function": {
            "name": "ExecCommand",
            "description": "Run a shell command",
            "parameters": { "type": "object", "properties": { "cmd": { "type": "string" } } }
        }
    }]);

    let cases: Vec<(&str, &str, String, u32)> = vec![
        (
            "minimal/auto/ctx0",
            "auto",
            serde_json::json!({
                "model": "auto",
                "messages": [{ "role": "user", "content": "Reply with exactly: PONG" }],
                "stream": true,
            })
            .to_string(),
            0,
        ),
        (
            "openai-envelope/auto/ctx300k",
            "auto",
            serde_json::json!({
                "model": "auto",
                "messages": [{ "role": "user", "content": "Reply with exactly: PONG" }],
                "tools": tools,
                "stream": true,
                "stream_options": { "include_usage": true },
            })
            .to_string(),
            300000,
        ),
        (
            "openai-envelope/Qwen3.8-Flash/ctx300k",
            "Qwen3.8-Flash",
            serde_json::json!({
                "model": "Qwen3.8-Flash",
                "messages": [{ "role": "user", "content": "Reply with exactly: PONG" }],
                "tools": tools,
                "stream": true,
                "stream_options": { "include_usage": true },
            })
            .to_string(),
            300000,
        ),
        (
            "openai-envelope/auto/no-stream",
            "auto",
            serde_json::json!({
                "model": "auto",
                "messages": [{ "role": "user", "content": "Reply with exactly: PONG" }],
                "stream": false,
            })
            .to_string(),
            0,
        ),
    ];

    for (label, model_key, body, ctx) in cases {
        let model_key = model_key.to_string();
        let pat = pat.clone();
        let client = client.clone();
        let label = label.to_string();
        let body_for_sign = body.clone();

        let outcome = runtime.block_on(async move {
            let signed = tokio::task::spawn_blocking(move || {
                let config = host_config(&pat, &model_key, ctx);
                qoder_body_override(&config, REQUEST_URL, &body_for_sign).map(|s| (config, s))
            })
            .await
            .expect("signing task joins");

            let Some((config, (signed_url, encrypted))) = signed else {
                return format!("{label} => SEAM NONE (unsigned)");
            };

            let builder = client
                .post(&signed_url)
                .header("Accept", "text/event-stream")
                .body(encrypted.clone());
            let builder =
                inject_qoder_headers(builder, REQUEST_URL, &config, &body, Some(&encrypted));
            match builder.send().await {
                Ok(response) => {
                    let status = response.status();
                    let text = response.text().await.unwrap_or_default();
                    // Trim to the gateway envelope, which carries the verdict.
                    let head: String = text.chars().take(260).collect();
                    format!("{label} => {status} (enc={}) :: {head}", encrypted.len())
                }
                Err(error) => format!("{label} => TRANSPORT ERROR {error}"),
            }
        });

        println!("[body-shape] {outcome}");
    }
}
