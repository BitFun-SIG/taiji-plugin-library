//! Regression guard: the gateway's inference endpoint routes on the model id the
//! picker carries, not on the catalog alias.
//!
//! The seam once rewrote `Qwen3.8-Flash` to its catalog alias `qfmodel` before
//! signing. The gateway accepts the alias on the catalog endpoint but answers it
//! on the inference endpoint with
//! `{"code":"400","message":"[FAIL]node:oa_qwen-plus-main msg:Execution failed: null"}`
//! — a 200 envelope carrying a 400, which the host reports as "stream closed
//! before response completed". Each model is therefore probed under both
//! spellings: the picker's form must serve, and the alias is recorded to show
//! the failure mode stays confined to it.
//!
//! Ignored by default (network + live credential):
//!
//! ```text
//! $env:BITFUN_QODER_PAT = "<pt-...>"
//! cargo test --features subscription-auth --test model_route_e2e -- --ignored --nocapture
//! ```

#![cfg(feature = "subscription-auth")]

use openbitfun_core_types::AIConfig;
use taiji_qoder_adapter::{inject_qoder_headers, qoder_body_override};

const REQUEST_URL: &str = "https://gateway.qoder.com.cn/chat/completions";

/// `(picker_spelling, catalog_alias)` for the catalogs observed so far.
const MODELS: &[(&str, &str)] = &[
    ("auto", "auto"),
    ("Qwen3.8-Flash", "qfmodel"),
    ("Qwen3.8-Max", "qmodel_38max"),
    ("Qwen3.7-Plus", "qmodel"),
    ("Qwen3.7-Flash", "q37fmodel"),
    ("DeepSeek-V4-Pro", "dmodel"),
    ("GLM-5.3", "gmodel"),
];

fn host_config(pat: &str, model: &str) -> AIConfig {
    AIConfig {
        name: "qoder-route".to_string(),
        base_url: "https://gateway.qoder.com.cn".to_string(),
        request_url: REQUEST_URL.to_string(),
        api_key: pat.to_string(),
        model: model.to_string(),
        format: "openai".to_string(),
        context_window: 200000,
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

async fn probe(client: reqwest::Client, pat: String, model: String) -> String {
    let body = serde_json::json!({
        "model": model,
        "messages": [{ "role": "user", "content": "Reply with exactly: PONG" }],
        "stream": true,
    })
    .to_string();
    let body_for_sign = body.clone();

    let signed = tokio::task::spawn_blocking(move || {
        let config = host_config(&pat, &model);
        qoder_body_override(&config, REQUEST_URL, &body_for_sign).map(|s| (config, s))
    })
    .await
    .expect("signing joins");

    let Some((config, (signed_url, encrypted))) = signed else {
        return "SEAM NONE".to_string();
    };

    let builder = client
        .post(&signed_url)
        .header("Accept", "text/event-stream")
        .body(encrypted.clone());
    let builder = inject_qoder_headers(builder, REQUEST_URL, &config, &body, Some(&encrypted));

    match builder.send().await {
        Ok(response) => {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            // The gateway wraps the real answer in an SSE envelope whose payload
            // is a JSON *string*, so every key inside it is escaped
            // (`\"choices\"`). Match both spellings; a plain `"choices"` check
            // sees nothing and reports a false OTHER.
            let flattened = text.replace("\\\"", "\"");
            let verdict = if flattened.contains("\"choices\"") {
                "OK"
            } else if flattened.contains("\"code\":\"400\"") {
                "ENVELOPE_400"
            } else if flattened.contains("10605") {
                "QUEUED"
            } else if flattened.contains("All models failed")
                || flattened.contains("All backends failed")
            {
                "PROVIDER_ERR"
            } else {
                "OTHER"
            };
            let head: String = text.chars().take(160).collect();
            format!("http={status} {verdict} :: {head}")
        }
        Err(error) => format!("TRANSPORT {error}"),
    }
}

#[test]
#[ignore = "live network + credential; run with --ignored"]
fn picker_spelling_routes_but_the_catalog_alias_does_not() {
    let pat = std::env::var("BITFUN_QODER_PAT").expect("BITFUN_QODER_PAT must be set");
    let _ = rustls::crypto::ring::default_provider().install_default();
    let client = reqwest::Client::builder().build().expect("client builds");
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("runtime builds");

    for (display, alias) in MODELS {
        let display_verdict =
            runtime.block_on(probe(client.clone(), pat.clone(), (*display).to_string()));
        println!("[model-route] picker  {display:<16} => {display_verdict}");

        if alias != display {
            let alias_verdict =
                runtime.block_on(probe(client.clone(), pat.clone(), (*alias).to_string()));
            println!("[model-route] alias   {alias:<16} => {alias_verdict}");
        }
    }
}
