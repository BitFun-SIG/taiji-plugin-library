//! End-to-end acceptance probe for the Qoder seam.
//!
//! Drives the *Rust* seam exactly as the host does — material derivation, then
//! `qoder_body_override` + `inject_qoder_headers` — and POSTs a real inference
//! request to the Qoder CN gateway.
//!
//! The PAT -> job-token exchange lives in the host's subscription-auth module
//! (the seam performs no credential IO), so this probe drives that exchange
//! itself to stand in for the host's login step.
//!
//! Ignored by default (network + live credential). Run explicitly:
//!
//! ```text
//! $env:BITFUN_QODER_PAT = "<pt-...>"
//! cargo test --features subscription-auth --test live_e2e -- --ignored --nocapture
//! ```
//!
//! The PAT is read from the environment and never written to disk.

#![cfg(feature = "subscription-auth")]

use futures_util::StreamExt;
use openbitfun_core_types::AIConfig;
use taiji_qoder_adapter::{
    QoderCredential, derive_login_materials, inject_qoder_headers, qoder_body_override,
    request_profile,
};

const GATEWAY_URL: &str = "https://gateway.qoder.com.cn/model/v1/chat/completions";
const OPENAPI_URL: &str = "https://openapi.qoder.com.cn";

fn base_config() -> AIConfig {
    AIConfig {
        name: "qoder-live".to_string(),
        base_url: "https://gateway.qoder.com.cn".to_string(),
        request_url: GATEWAY_URL.to_string(),
        api_key: String::new(),
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

/// Installs the workspace's provider-neutral rustls backend before use.
fn live_client() -> reqwest::Client {
    let _ = rustls::crypto::ring::default_provider().install_default();
    reqwest::Client::builder()
        .build()
        .expect("live HTTP client builds")
}

/// The host's login step, reproduced here so the probe is self-contained:
/// PAT -> job token, then the account profile the material derivation needs.
async fn host_login_step(client: &reqwest::Client, pat: &str) -> (String, serde_json::Value) {
    let exchange = client
        .post(format!("{OPENAPI_URL}/api/v1/jobToken/exchange"))
        .json(&serde_json::json!({ "personal_token": pat }))
        .send()
        .await
        .expect("job-token exchange reaches the OpenAPI host");
    let status = exchange.status();
    let body: serde_json::Value = exchange.json().await.expect("exchange body is JSON");
    assert!(
        status.is_success(),
        "job-token exchange must answer 2xx, got {status}: {body}"
    );
    let job_token = body
        .get("token")
        .and_then(|v| v.as_str())
        .or_else(|| {
            body.get("data")
                .and_then(|d| d.get("token"))
                .and_then(|v| v.as_str())
        })
        .filter(|t| !t.is_empty())
        .expect("exchange carries a job token")
        .to_string();

    let userinfo = client
        .get(format!("{OPENAPI_URL}/api/v1/userinfo"))
        .header("authorization", format!("Bearer {job_token}"))
        .send()
        .await
        .expect("userinfo reaches the OpenAPI host");
    let status = userinfo.status();
    let profile: serde_json::Value = userinfo.json().await.expect("userinfo body is JSON");
    assert!(
        status.is_success(),
        "userinfo must answer 2xx, got {status}: {profile}"
    );
    (job_token, profile)
}

#[tokio::test]
#[ignore = "live network + credential; run with --ignored"]
async fn live_inference_reaches_gateway_with_200() {
    let pat = std::env::var("BITFUN_QODER_PAT").expect("BITFUN_QODER_PAT must be set");
    assert!(!pat.is_empty(), "BITFUN_QODER_PAT must not be empty");

    let client = live_client();

    // 1) Host login step: PAT -> job token -> account profile.
    let (job_token, profile) = host_login_step(&client, &pat).await;
    println!("[1] login ok: job_token len={}", job_token.len());

    // 2) Material derivation: the seam turns the resolved uid plus the account
    // attributes into the reserved `custom_headers` map the signature reads.
    let uid = profile
        .get("id")
        .and_then(|v| v.as_str())
        .or_else(|| profile.get("user_id").and_then(|v| v.as_str()))
        .filter(|uid| !uid.is_empty())
        .expect("profile carries the account id");
    let materials = derive_login_materials(
        uid,
        profile.get("organization_id").and_then(|v| v.as_str()),
        profile.get("organization_tags"),
        // The gateway distinguishes an absent policy field from an explicit
        // `false`, so the resolved value is forwarded verbatim.
        profile.get("data_policy_agreed"),
    )
    .expect("signing materials derive from the resolved uid");
    println!(
        "[2] materials ok: uid_present={} key_present={}",
        materials.contains_key("x-taiji-qoder-uid"),
        materials.contains_key("x-taiji-qoder-key"),
    );
    assert!(materials.contains_key("x-taiji-qoder-uid"));
    assert!(materials.contains_key("x-taiji-qoder-key"));

    // 3) Body seam: signed URL + encrypted body.
    let credential = QoderCredential {
        job_token: job_token.clone(),
        personal_token: pat.clone(),
        expires_at_ms: 0,
        materials: materials.clone(),
    };
    let profile_out = request_profile(&credential);
    let mut config = base_config();
    config.api_key = profile_out.api_key.clone();
    config.custom_headers = Some(materials);
    let body = serde_json::json!({
        "model": "auto",
        "messages": [{ "role": "user", "content": "Reply with exactly: PONG" }],
        "stream": true,
    })
    .to_string();
    let (signed_url, encrypted_body) =
        qoder_body_override(&config, GATEWAY_URL, &body).expect("seam encrypts the body");
    println!(
        "[3] seam: signed_url={} encrypted_body_len={}",
        signed_url,
        encrypted_body.len()
    );
    assert!(!encrypted_body.is_empty());
    // Objective ruler 1: the gateway's encrypted payload is block-aligned.
    assert_eq!(
        encrypted_body.len() % 16,
        0,
        "encrypted body must be block-aligned, got {}",
        encrypted_body.len()
    );

    // 4) Header seam, then POST through the real transport.
    let builder = client
        .post(&signed_url)
        .header("Accept", "text/event-stream")
        .body(encrypted_body.clone());
    let builder = inject_qoder_headers(builder, GATEWAY_URL, &config, &body, Some(&encrypted_body));
    let response = builder
        .send()
        .await
        .expect("gateway accepts the connection");
    let status = response.status();
    println!("[4] infer status={status}");
    assert_eq!(
        status.as_u16(),
        200,
        "gateway must answer 200, got {status}"
    );

    // The gateway holds the SSE connection open (keep-alive), so `text()` would
    // block until the read timeout and then report an empty body. Read
    // incrementally and stop at the `[DONE]` sentinel instead.
    let text = read_until_done(response).await;
    let frames: Vec<&str> = text.lines().filter(|l| l.starts_with("data:")).collect();
    println!("[5] data frames={}", frames.len());
    assert!(
        text.contains("\"choices\"") || text.contains("data:"),
        "200 body must carry the gateway envelope, got: {text}"
    );
    assert!(
        text.contains("[DONE]"),
        "the stream must terminate with the [DONE] sentinel, got: {text}"
    );

    // 5) Persist the raw round trip for the receipt. The request side is
    // redacted: the COSY signature and encrypted body are dropped, the
    // plaintext body that produced them is kept.
    if let Ok(path) = std::env::var("QODER_SAMPLE_OUT") {
        let report = format!(
            "# Qoder seam live round trip (redacted)\n\
             \n\
             ## Request\n\
             POST {signed_url}\n\
             Authorization: Bearer COSY.<redacted>\n\
             Content-Type: application/octet-stream\n\
             plaintext_body_before_encryption: {body}\n\
             encrypted_body_len: {}\n\
             \n\
             ## Response\n\
             status: {status}\n\
             data_frames: {}\n\
             \n\
             ## SSE stream (verbatim)\n\
             {text}\n",
            encrypted_body.len(),
            frames.len(),
        );
        std::fs::write(&path, report).expect("artifact written");
        println!("[6] artifact -> {path}");
    }
}

/// Collects the SSE body incrementally, stopping at the `[DONE]` sentinel or
/// when the stream ends.
///
/// The gateway keeps the connection open after the final frame, so waiting for
/// the socket to close would hang until the read timeout. Failures are reported
/// with whatever was already received, never swallowed into an empty string.
async fn read_until_done(response: reqwest::Response) -> String {
    let mut stream = response.bytes_stream();
    let mut collected = String::new();
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        match tokio::time::timeout_at(deadline, stream.next()).await {
            Err(_) => panic!(
                "timed out waiting for the Qoder stream after 30s; received so far: {collected}"
            ),
            Ok(None) => break,
            Ok(Some(Err(error))) => {
                panic!("Qoder stream read failed: {error}; received so far: {collected}")
            }
            Ok(Some(Ok(bytes))) => {
                collected.push_str(&String::from_utf8_lossy(&bytes));
                if collected.contains("[DONE]") {
                    break;
                }
            }
        }
    }
    collected
}
