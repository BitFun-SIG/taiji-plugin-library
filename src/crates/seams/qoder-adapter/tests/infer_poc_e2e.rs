//! Inference-level proof of concept: sign and POST a chat request exactly the
//! way the reference implementation does, then report what the gateway answers.
//!
//! This is the smallest complete reproduction of the host's failing path: one
//! PAT -> job token exchange, one wasm signature, one POST to the signed
//! `agent_chat_generation` URL.
//!
//! ```text
//! $env:BITFUN_QODER_PAT = "<pt-...>"
//! cargo test --features subscription-auth --test infer_poc_e2e -- --ignored --nocapture
//! ```

#![cfg(feature = "subscription-auth")]

use taiji_qoder_adapter::{inject_qoder_headers, qoder_body_override};

const REQUEST_URL: &str = "https://gateway.qoder.com.cn/chat/completions";
const MODELS: &[&str] = &["Qwen3.8-Flash", "auto"];

#[test]
#[ignore = "live network + credential; run with --ignored"]
fn signed_inference_reaches_the_gateway() {
    let pat = std::env::var("BITFUN_QODER_PAT").expect("BITFUN_QODER_PAT must be set");
    let _ = rustls::crypto::ring::default_provider().install_default();

    println!(
        "[infer-poc] machine_id = {}",
        taiji_qoder_adapter::qoder_machine_id()
    );

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("runtime builds");
    let client = reqwest::Client::builder().build().expect("client builds");

    for model in MODELS {
        let model = (*model).to_string();
        let verdict = runtime.block_on(async {
            let body = serde_json::json!({
                "max_tokens": 32000,
                "messages": [{
                    "content": "Call the get_weather tool for city=Beijing. Do not answer with plain text.",
                    "role": "user"
                }],
                "model": model,
                "stream": true,
                "tools": [{
                    "type": "function",
                    "function": {
                        "name": "get_weather",
                        "description": "Get the current weather for a city",
                        "parameters": {
                            "type": "object",
                            "properties": { "city": { "type": "string" } },
                            "required": ["city"]
                        }
                    }
                }],
            })
            .to_string();
            let body_for_sign = body.clone();
            let pat = pat.clone();
            let model_for_config = model.clone();

            let config = openbitfun_core_types::AIConfig {
                name: "qoder-poc".to_string(),
                base_url: "https://gateway.qoder.com.cn".to_string(),
                request_url: REQUEST_URL.to_string(),
                api_key: pat,
                model: model_for_config,
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
            };

            let signed = tokio::task::spawn_blocking({
                let config = config.clone();
                move || qoder_body_override(&config, REQUEST_URL, &body_for_sign)
            })
            .await
            .expect("signing joins");

            let Some((signed_url, encrypted)) = signed else {
                return "SEAM NONE".to_string();
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
                    let head: String = text.chars().take(220).collect();
                    format!("http={status} :: {head}")
                }
                Err(error) => format!("TRANSPORT {error}"),
            }
        });
        println!("[infer-poc] {model} => {verdict}");
    }
}
