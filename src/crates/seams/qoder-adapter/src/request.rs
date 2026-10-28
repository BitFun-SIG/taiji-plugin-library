//! COSY request signing (the seam's `qoder_signed_request` body).
//!
//! The signing materials (`uid`, `encrypt_user_info`, `key`) normally arrive
//! through `AIConfig::custom_headers` under three reserved keys owned by this
//! module. When the config instead carries the user's personal access token in
//! the ordinary API-key field, this module derives them once through
//! `login::materials_for_pat` and caches the result, so the user supplies a
//! single key exactly as they would for any other provider.

use std::collections::HashMap;

use openbitfun_core_types::AIConfig;

use crate::QoderSignedRequest;
use crate::machine::qoder_machine_id;
use crate::wasm::QoderWasm;

/// Gateway host used as the wasm signing endpoint (Qoder CN production).
pub(crate) const MODEL_BASE_URL: &str = "https://gateway.qoder.com.cn";

/// Full inference URL the caller POSTs the encrypted body to. Kept as part of
/// the documented endpoint table even though the signed URL is returned by the
/// wasm; referenced by the host's endpoint wiring.
#[allow(dead_code)]
pub(crate) const MODEL_REQUEST_URL: &str = "https://gateway.qoder.com.cn/model/v1/chat/completions";

/// Public OAuth client id of the Qoder CN CLI; not a secret.
#[allow(dead_code)]
pub(crate) const CLIENT_ID: &str = "e883ade2-e6e3-4d6d-adf7-f92ceff5fdcb";

/// Reserved `custom_headers` key carrying the Qoder `uid`.
pub(crate) const X_TAIJI_QODER_UID: &str = "x-taiji-qoder-uid";
/// Reserved `custom_headers` key carrying the wasm `encrypt_user_info`.
pub(crate) const X_TAIJI_QODER_USER_INFO: &str = "x-taiji-qoder-user-info";
/// Reserved `custom_headers` key carrying the wasm signing `key`.
pub(crate) const X_TAIJI_QODER_KEY: &str = "x-taiji-qoder-key";

/// The wasm context version string the CLI pins.
pub const QODER_COSY_VERSION: &str = "1.1.23";

/// Returns the value to sign for `model`, which is the host's own model id.
///
/// The gateway's inference endpoint routes on the **display name** the model
/// picker carries (`Qwen3.8-Flash`). Its catalog also publishes an internal
/// alias (`qfmodel`) for the same entry, but the two are not interchangeable at
/// this endpoint: signing the alias is answered with
/// `{"code":"400","message":"[FAIL]node:oa_qwen-plus-main msg:Execution failed: null"}`
/// while the display name returns a normal `choices` stream.
///
/// This is the reference implementation's behaviour too — it passes
/// `client.config.model` straight through to `prepareInferRequest`. An earlier
/// revision of this seam rewrote the display name to the catalog alias; that
/// rewrite is what broke `Qwen3.8-Flash`.
///
/// The value is trimmed, and an empty result is returned unchanged so the caller
/// can treat it as "no model".
pub fn gateway_model_key(display_name: &str) -> &str {
    display_name.trim()
}

/// Reads the three signing materials from the config. This is the single place
/// the material channel is touched: swapping the channel changes only this
/// function.
fn signature_materials(config: &AIConfig) -> Option<(&str, &str, &str)> {
    let headers = config.custom_headers.as_ref()?;
    let uid = headers.get(X_TAIJI_QODER_UID)?.as_str();
    let user_info = headers.get(X_TAIJI_QODER_USER_INFO)?.as_str();
    let key = headers.get(X_TAIJI_QODER_KEY)?.as_str();
    if uid.is_empty() || user_info.is_empty() || key.is_empty() {
        return None;
    }
    Some((uid, user_info, key))
}

/// Resolves the signing materials for `config`, deriving them from the user's
/// personal access token when the config does not already carry them.
///
/// This is what lets the user supply a single Qoder PAT in the ordinary API-key
/// field, exactly as they would for any other provider: the first signed request
/// performs the PAT -> job-token exchange, derives the wasm materials, and keeps
/// them in a process-wide cache so later requests pay nothing. A non-PAT key
/// (`jt-`, or a value with no `pt-` prefix) is left alone, so a caller that
/// pre-populated `custom_headers` — or pasted an already-exchanged token — keeps
/// its previous behaviour.
fn resolve_materials(config: &AIConfig) -> Option<HashMap<String, String>> {
    if let Some((uid, user_info, key)) = signature_materials(config) {
        return Some(HashMap::from([
            (X_TAIJI_QODER_UID.to_string(), uid.to_string()),
            (X_TAIJI_QODER_USER_INFO.to_string(), user_info.to_string()),
            (X_TAIJI_QODER_KEY.to_string(), key.to_string()),
        ]));
    }
    let token = config.api_key.trim();
    if token.is_empty() || !token.starts_with("pt-") {
        return None;
    }
    crate::login::materials_for_pat(token)
}

/// Signs an inference request body, mirroring the CLI's
/// `prepareInferRequest(endpoint, body, model_key, source)` chain.
///
/// Returns `None` when the URL is not a Qoder gateway URL, the signing
/// materials cannot be resolved, or the wasm fails — the caller then proceeds
/// with the unsigned legacy path.
pub(crate) fn signed_request(
    config: &AIConfig,
    url: &str,
    body: &str,
    model_key: &str,
) -> Option<QoderSignedRequest> {
    if !crate::is_qoder_url(url) {
        return None;
    }
    let materials = resolve_materials(config)?;
    let uid = materials.get(X_TAIJI_QODER_UID)?;
    let encrypt_user_info = materials.get(X_TAIJI_QODER_USER_INFO)?;
    let key = materials.get(X_TAIJI_QODER_KEY)?;
    let user_info = serde_json::json!({
        "uid": uid,
        "encrypt_user_info": encrypt_user_info,
        "key": key,
    })
    .to_string();

    let mut runtime = QoderWasm::instantiate()?;
    let machine_id = qoder_machine_id();
    let ctx = runtime.context_new(
        &machine_id,
        QODER_COSY_VERSION,
        &user_info,
        r#"{"client_type":5}"#,
    )?;
    // CLI `vVi`: prepareInferRequest(endpoint, body, model_key, source) maps to
    // wasm `(endpoint, path_or_body = body, body = model_key, headers = source)`.
    // The model id travels verbatim: the inference endpoint routes on the name
    // the picker carries, not on the catalog alias.
    let gateway_key = gateway_model_key(model_key);
    let prepared = runtime.prepare_infer_request(
        ctx,
        MODEL_BASE_URL,
        body,
        Some(gateway_key),
        Some("system"),
    )?;
    let headers = prepared.headers()?;
    let body = prepared.body.unwrap_or_else(|| body.as_bytes().to_vec());
    Some(QoderSignedRequest {
        url: prepared.url,
        headers,
        body,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The inference endpoint routes on the display name. Signing the catalog
    /// alias instead (`qfmodel`) is answered with
    /// `[FAIL]node:oa_qwen-plus-main msg:Execution failed: null`, so the value
    /// the picker carries must reach the signer unrewritten.
    #[test]
    fn display_names_are_signed_verbatim() {
        assert_eq!(gateway_model_key("Qwen3.8-Flash"), "Qwen3.8-Flash");
        assert_eq!(gateway_model_key("Qwen3.8-Max"), "Qwen3.8-Max");
        // Surrounding whitespace from a pasted value is dropped.
        assert_eq!(gateway_model_key(" Qwen3.8-Flash "), "Qwen3.8-Flash");
    }

    /// A catalog alias, `auto`, an unknown name, and an empty value all pass
    /// through unchanged, so a caller that already speaks the gateway's language
    /// keeps its previous behaviour.
    #[test]
    fn catalog_aliases_and_free_form_values_pass_through_unchanged() {
        assert_eq!(gateway_model_key("auto"), "auto");
        assert_eq!(gateway_model_key("some-future-model"), "some-future-model");
        assert_eq!(gateway_model_key(""), "");
    }
}
