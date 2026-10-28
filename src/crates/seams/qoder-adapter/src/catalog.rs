//! Qoder model catalog decoding (the seam's `qoder_model_catalog` body).
//!
//! The gateway may return an encrypted payload; decryption is attempted first
//! and a plaintext body is used as-is when it fails (the CLI's `NsA` fallback
//! semantics). Parsing accepts the `chat` scene, a `models` array, or a
//! top-level array. The stored id is the display name, except `auto` which keeps
//! its routing key.

use crate::wasm::QoderWasm;

/// Decodes the gateway model catalog into model ids.
///
/// Returns `None` when the payload cannot be parsed into any known shape.
pub(crate) fn model_catalog(raw_body: &str) -> Option<Vec<String>> {
    let decrypted = match QoderWasm::instantiate() {
        Some(mut runtime) => runtime
            .decrypt_server_response(raw_body)
            .unwrap_or_else(|| raw_body.to_string()),
        None => raw_body.to_string(),
    };
    let payload: serde_json::Value = serde_json::from_str(&decrypted).ok()?;
    let scene = payload
        .get("chat")
        .or_else(|| payload.get("models"))
        .or_else(|| payload.as_array().map(|_| &payload))?;
    let items = match scene {
        serde_json::Value::Array(items) => items,
        _ => return None,
    };
    let models: Vec<String> = items
        .iter()
        .filter_map(|item| {
            let key = item.get("key").and_then(|value| value.as_str())?;
            let display = item.get("display_name").and_then(|value| value.as_str());
            // The stored id is the display name, except `auto` which keeps its
            // routing key (the CLI's default model value).
            let id = match display {
                Some(display) if key != "auto" => display.to_string(),
                _ => key.to_string(),
            };
            Some(id)
        })
        .collect();
    if models.is_empty() {
        return None;
    }
    Some(models)
}
