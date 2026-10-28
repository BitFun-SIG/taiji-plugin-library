//! Isolates which request characteristic makes the gateway reject `qfmodel`.
//!
//! The CLI signs `qfmodel` successfully; this seam signs the same key and gets
//! `400 [FAIL]node:oa_qwen-plus-main msg:Execution failed`. The candidates are
//! the body fields the host adds that the CLI does not send. This probe varies
//! them one at a time so the culprit is identified rather than guessed.
//!
//! ```text
//! $env:BITFUN_QODER_PAT = "<pt-...>"
//! cargo test --features subscription-auth --test isolate_ctx -- --ignored --nocapture
//! ```

#![cfg(feature = "subscription-auth")]

use openbitfun_core_types::AIConfig;
use taiji_qoder_adapter::{inject_qoder_headers, qoder_body_override};

const GATEWAY_URL: &str = "https://gateway.qoder.com.cn";
const REQUEST_URL: &str = "https://gateway.qoder.com.cn/chat/completions";

fn config(pat: &str, model: &str, ctx: u32) -> AIConfig {
    AIConfig {
        name: "qoder".to_string(),
        base_url: GATEWAY_URL.to_string(),
        request_url: REQUEST_URL.to_string(),
        api_key: pat.to_string(),
        model: model.to_string(),
        format: "openai".to_string(),
        context_window: ctx,
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
fn isolate_the_rejected_field() {
    let pat = std::env::var("BITFUN_QODER_PAT").expect("BITFUN_QODER_PAT must be set");
    let _ = rustls::crypto::ring::default_provider().install_default();
    let client = reqwest::Client::builder().build().expect("client builds");
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("probe runtime builds");

    // (label, context_window, body-extra JSON)
    let cases: Vec<(&str, u32, serde_json::Value)> = vec![
        ("bare", 0, serde_json::json!({})),
        ("ctx180k", 180000, serde_json::json!({})),
        ("ctx300k", 300000, serde_json::json!({})),
        (
            "ctx180k+stream_options",
            180000,
            serde_json::json!({ "stream_options": { "include_usage": true } }),
        ),
        (
            "ctx180k+tools",
            180000,
            serde_json::json!({ "tools": [{ "type": "function", "function": { "name": "ExecCommand", "description": "Run", "parameters": { "type": "object", "properties": {} } } }] }),
        ),
        (
            "agent-shaped(qfmodel param set)",
            180000,
            serde_json::json!({
                "parameters": {},
                "request_id": "probe-request",
                "session_id": "probe-session",
            }),
        ),
    ];

    for (label, ctx, extra) in cases {
        let pat = pat.clone();
        let client = client.clone();
        let label = label.to_string();

        let outcome = runtime.block_on(async move {
            let mut body_value = serde_json::json!({
                "model": "Qwen3.8-Flash",
                "messages": [{ "role": "user", "content": "Reply with exactly: PONG" }],
                "stream": true,
            });
            if let (Some(dst), Some(src)) = (body_value.as_object_mut(), extra.as_object()) {
                for (k, v) in src {
                    dst.insert(k.clone(), v.clone());
                }
            }
            let body = body_value.to_string();
            let body_for_sign = body.clone();

            let signed = tokio::task::spawn_blocking(move || {
                let config = config(&pat, "Qwen3.8-Flash", ctx);
                qoder_body_override(&config, REQUEST_URL, &body_for_sign).map(|s| (config, s))
            })
            .await
            .expect("signing task joins");

            let Some((config, (signed_url, encrypted))) = signed else {
                return format!("{label} => SEAM NONE");
            };
            let builder = client
                .post(&signed_url)
                .header("Accept", "text/event-stream")
                .body(encrypted.clone());
            let builder =
                inject_qoder_headers(builder, REQUEST_URL, &config, &body, Some(&encrypted));
            match builder.send().await {
                Ok(r) => {
                    let status = r.status();
                    let text = r.text().await.unwrap_or_default();
                    let head: String = text.chars().take(200).collect();
                    format!("{label} => {status} :: {head}")
                }
                Err(e) => format!("{label} => TRANSPORT {e}"),
            }
        });

        println!("[isolate] {outcome}");
    }
}
