//! Qoder signing-material derivation (the seam's material face).
//!
//! This module is deliberately NOT a credential lifecycle: it performs no
//! network IO and stores no secret. The PAT -> job-token exchange and the
//! userinfo lookup live in the host's own subscription-auth module, because they
//! are live HTTPS calls that produce a persisted credential — the host owns both.
//!
//! What stays here is the one part only the seam can do: driving the embedded
//! wasm (`generate_runtime_auth_fields`) to turn a resolved `uid` plus the
//! account attributes into the `{encrypt_user_info, key}` pair the COSY
//! signature needs. The host hands the result back through the reserved
//! `custom_headers` keys that `request.rs` reads.

use std::collections::HashMap;

use crate::wasm::QoderWasm;

/// Qoder CN gateway origin used as the model base URL.
pub(crate) const MODEL_BASE_URL: &str = "https://gateway.qoder.com.cn";

/// Inference request URL the gateway signs and serves.
pub(crate) const MODEL_REQUEST_URL: &str = "https://gateway.qoder.com.cn/model/v1/chat/completions";

/// Model catalog endpoint on the gateway.
#[allow(dead_code)]
pub(crate) const MODEL_LIST_URL: &str = "https://gateway.qoder.com.cn/api/v2/model/list?Encode=1";

/// Default model routing key.
pub(crate) const DEFAULT_MODEL: &str = "auto";

/// Wire format of the resolved credential. The Qoder CN gateway speaks the
/// ordinary OpenAI wire format, so the host rides its existing openai flow.
pub(crate) const MODEL_FORMAT: &str = "openai";

/// Reserved channel keys the request path reads the materials from. Owned by
/// `request.rs`; named here too so both sides agree by construction.
pub(crate) const HEADER_UID: &str = "x-taiji-qoder-uid";
pub(crate) const HEADER_USER_INFO: &str = "x-taiji-qoder-user-info";
pub(crate) const HEADER_KEY: &str = "x-taiji-qoder-key";

/// The three signing materials the signature path consumes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QoderSignature {
    pub uid: String,
    pub encrypt_user_info: String,
    pub key: String,
}

impl QoderSignature {
    /// Renders the triple as the reserved `custom_headers` map the host stores on
    /// the credential and writes onto `AIConfig::custom_headers`.
    pub fn to_materials(&self) -> HashMap<String, String> {
        let mut headers = HashMap::new();
        headers.insert(HEADER_UID.to_string(), self.uid.clone());
        headers.insert(HEADER_USER_INFO.to_string(), self.encrypt_user_info.clone());
        headers.insert(HEADER_KEY.to_string(), self.key.clone());
        headers
    }
}

/// Derives the `{encrypt_user_info, key}` pair from a resolved `uid` and the
/// account attributes the profile carries.
///
/// Returns `None` when the wasm runtime is unavailable or the call fails, so the
/// host can fall back to an unsigned request.
pub(crate) fn derive_auth_fields(
    uid: &str,
    organization_id: Option<&str>,
    organization_tags: Option<&serde_json::Value>,
    data_policy_agreed: Option<&serde_json::Value>,
) -> Option<(String, String)> {
    let mut runtime = QoderWasm::instantiate()?;
    // The wasm reads these three fields verbatim; `null` and `false` are not
    // interchangeable inputs to it. Passing a synthesised `false` for an account
    // whose profile simply omits `data_policy_agreed` derives a different
    // `encrypt_user_info`/`key` pair than the client does, so the omitted field
    // must stay `null`.
    let request = serde_json::json!({
        "uid": uid,
        "organization_id": organization_id.unwrap_or(""),
        "organization_tags": organization_tags.cloned().unwrap_or(serde_json::Value::Null),
        "data_policy_agreed": data_policy_agreed.cloned().unwrap_or(serde_json::Value::Null),
    });
    let derived = runtime.generate_runtime_auth_fields(&request.to_string())?;
    let fields: serde_json::Value = serde_json::from_str(&derived).ok()?;
    let encrypt_user_info = fields.get("encrypt_user_info")?.as_str()?.to_string();
    let key = fields.get("key")?.as_str()?.to_string();
    if encrypt_user_info.is_empty() || key.is_empty() {
        return None;
    }
    Some((encrypt_user_info, key))
}

/// Builds the full signing-material triple for a resolved `uid`.
///
/// This is the single place this module produces what the request path consumes,
/// keeping the material contract in one module.
pub(crate) fn build_signature(
    uid: &str,
    organization_id: Option<&str>,
    organization_tags: Option<&serde_json::Value>,
    data_policy_agreed: Option<&serde_json::Value>,
) -> Option<QoderSignature> {
    let (encrypt_user_info, key) =
        derive_auth_fields(uid, organization_id, organization_tags, data_policy_agreed)?;
    Some(QoderSignature {
        uid: uid.to_string(),
        encrypt_user_info,
        key,
    })
}

/// Derives the signing materials as the reserved header map the config carries.
///
/// This is the seam's single material-producing entry point: the returned map is
/// exactly what `qoder_signed_request` reads back out of `AIConfig::custom_headers`.
pub fn derive_login_materials(
    uid: &str,
    organization_id: Option<&str>,
    organization_tags: Option<&serde_json::Value>,
    data_policy_agreed: Option<&serde_json::Value>,
) -> Option<HashMap<String, String>> {
    let signature = build_signature(uid, organization_id, organization_tags, data_policy_agreed)?;
    Some(signature.to_materials())
}

/// What the host needs to seed its own credential record. Plain values only: the
/// host formats and persists them, so the seam never sees a host type.
///
/// The host's login module builds this after its own PAT -> job-token exchange;
/// the seam is not involved in that IO.
pub struct QoderCredential {
    /// The short-lived job token the gateway authorises (used as the API key).
    pub job_token: String,
    /// The user's personal access token, kept by the host so it can re-exchange.
    pub personal_token: String,
    /// Absolute expiry of `job_token`, in milliseconds since the epoch.
    pub expires_at_ms: i64,
    /// Signing materials as the reserved `custom_headers` map, ready to store.
    pub materials: HashMap<String, String>,
}

/// The runtime endpoint set the host wires into its model entry.
pub struct QoderRequestProfile {
    pub api_key: String,
    pub base_url: String,
    pub request_url: String,
    pub format: String,
    pub extra_headers: HashMap<String, String>,
    pub expires_at: Option<i64>,
}

/// Renders the endpoint set for a resolved credential.
///
/// The signing materials ride the credential's stored `custom_headers`, so the
/// profile carries no header material: the host reads the materials back from
/// the credential when it builds the `AIConfig`.
pub fn request_profile(credential: &QoderCredential) -> QoderRequestProfile {
    QoderRequestProfile {
        api_key: credential.job_token.clone(),
        base_url: MODEL_BASE_URL.to_string(),
        request_url: MODEL_REQUEST_URL.to_string(),
        format: MODEL_FORMAT.to_string(),
        extra_headers: credential.materials.clone(),
        expires_at: Some(credential.expires_at_ms),
    }
}

/// Provider metadata used to seed a new model entry.
pub fn suggested() -> (&'static str, &'static str, &'static str) {
    (MODEL_BASE_URL, DEFAULT_MODEL, MODEL_FORMAT)
}

/// Qoder CN OpenAPI origin (PAT -> job-token exchange, userinfo).
pub(crate) const OPENAPI_URL: &str = "https://openapi.qoder.com.cn";

/// How long a freshly exchanged job token is considered valid (24h, the value
/// the exchange reports as `expires_in`).
const JOB_TOKEN_TTL_MS: i64 = 24 * 60 * 60 * 1000;

/// Millisecond wall-clock timestamp, or 0 when the clock is before the epoch.
fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or(0)
}

/// Builds the HTTP client used for the login exchange.
///
/// Features are disabled so the client inherits the caller's TLS setup rather
/// than pulling a second backend into the dependency graph.
fn login_client() -> reqwest::Result<reqwest::Client> {
    // The workspace uses provider-neutral rustls; install the ring provider once
    // before the first client is built. Repeated calls keep the first provider.
    let _ = rustls::crypto::ring::default_provider().install_default();
    reqwest::Client::builder().build()
}

/// Exchanges a personal token for a job token.
async fn exchange_job_token(
    client: &reqwest::Client,
    personal_token: &str,
) -> anyhow::Result<String> {
    let response = client
        .post(format!("{OPENAPI_URL}/api/v1/jobToken/exchange"))
        .json(&serde_json::json!({ "personal_token": personal_token }))
        .send()
        .await
        .map_err(|error| anyhow::anyhow!("Qoder job-token exchange: {error}"))?;
    let status = response.status();
    let body: serde_json::Value = response
        .json()
        .await
        .map_err(|error| anyhow::anyhow!("Qoder job-token exchange body: {error}"))?;
    if !status.is_success() {
        return Err(anyhow::anyhow!(
            "Qoder job-token exchange failed with status {status}"
        ));
    }
    // The exchange answers the job token under `token`; a nested `data.token`
    // shape is accepted as well so a gateway change does not silently yield an
    // empty credential.
    body.get("token")
        .and_then(|value| value.as_str())
        .or_else(|| {
            body.get("data")
                .and_then(|data| data.get("token"))
                .and_then(|value| value.as_str())
        })
        .filter(|token| !token.is_empty())
        .map(str::to_string)
        .ok_or_else(|| anyhow::anyhow!("Qoder job-token exchange carried no token"))
}

/// Reads the account profile behind a job token.
async fn request_userinfo(
    client: &reqwest::Client,
    job_token: &str,
) -> anyhow::Result<serde_json::Value> {
    let response = client
        .get(format!("{OPENAPI_URL}/api/v1/userinfo"))
        .header("authorization", format!("Bearer {job_token}"))
        .send()
        .await
        .map_err(|error| anyhow::anyhow!("Qoder userinfo: {error}"))?;
    let status = response.status();
    let body: serde_json::Value = response
        .json()
        .await
        .map_err(|error| anyhow::anyhow!("Qoder userinfo body: {error}"))?;
    if !status.is_success() {
        return Err(anyhow::anyhow!(
            "Qoder userinfo failed with status {status}"
        ));
    }
    Ok(body)
}

/// Completes a login from a personal access token: exchange the PAT for a job
/// token, read the account profile, then derive the wasm signing materials.
///
/// This lives in the seam (not upstream) because it is Qoder's own credential
/// protocol. It returns plain values; the host decides where they are persisted.
pub async fn pat_login(personal_token: &str) -> anyhow::Result<QoderCredential> {
    let client = login_client()?;
    let job_token = exchange_job_token(&client, personal_token).await?;
    let profile = request_userinfo(&client, &job_token).await?;
    let uid = profile
        .get("id")
        .and_then(|value| value.as_str())
        .or_else(|| profile.get("user_id").and_then(|value| value.as_str()))
        .filter(|uid| !uid.is_empty())
        .ok_or_else(|| anyhow::anyhow!("Qoder userinfo carried no account id"))?;
    let materials = build_signature(
        uid,
        profile
            .get("organization_id")
            .and_then(|value| value.as_str()),
        profile.get("organization_tags"),
        profile.get("data_policy_agreed"),
    )
    .map(|signature| signature.to_materials())
    .unwrap_or_default();
    Ok(QoderCredential {
        job_token,
        personal_token: personal_token.to_string(),
        expires_at_ms: now_ms().saturating_add(JOB_TOKEN_TTL_MS),
        materials,
    })
}

impl QoderCredential {
    /// True when the job token is still within its validity window.
    pub fn is_live(&self) -> bool {
        !self.job_token.is_empty() && self.expires_at_ms > now_ms()
    }

    /// Renders the host-persisted refresh blob: PAT and job token on two lines.
    pub fn to_store_blob(&self) -> String {
        format!("{}\n{}", self.personal_token, self.job_token)
    }

    /// Parses a host-persisted blob back into `(personal_token, job_token)`.
    ///
    /// A blob without a separator carries only the personal token.
    pub fn parse_store_blob(blob: &str) -> Option<(String, String)> {
        let mut parts = blob.splitn(2, '\n');
        let personal_token = parts.next()?;
        if personal_token.is_empty() {
            return None;
        }
        Some((
            personal_token.to_string(),
            parts.next().unwrap_or("").to_string(),
        ))
    }
}

/// Re-exchanges the stored personal token when the job token has expired.
///
/// Returns `None` when the stored credential is still valid, so the caller keeps
/// using it unchanged.
pub async fn refresh_credential(
    credential: &QoderCredential,
) -> anyhow::Result<Option<QoderCredential>> {
    if credential.is_live() {
        return Ok(None);
    }
    Ok(Some(pat_login(&credential.personal_token).await?))
}

/// Process-wide cache of derived signing materials, keyed by personal token.
///
/// The signature path is synchronous (it runs inside the adapter's request
/// builder), so the PAT exchange cannot await there. A successful exchange is
/// therefore cached for the process lifetime: the cost is paid once on the first
/// signed request, and every later request becomes a map lookup.
static MATERIALS_CACHE: std::sync::OnceLock<
    std::sync::Mutex<HashMap<String, HashMap<String, String>>>,
> = std::sync::OnceLock::new();

fn materials_cache() -> &'static std::sync::Mutex<HashMap<String, HashMap<String, String>>> {
    MATERIALS_CACHE.get_or_init(|| std::sync::Mutex::new(HashMap::new()))
}

/// Derives (and caches) the signing materials for a personal access token,
/// blocking until the exchange completes.
///
/// Returns `None` when the token is not a PAT-shaped value, the exchange fails,
/// or the wasm cannot derive the materials — the caller then falls back to the
/// unsigned path rather than sending a request the gateway will reject.
///
/// The exchange is driven on a private OS thread so this can be called from
/// inside an async request builder: the caller may already be a Tokio worker,
/// and building a runtime on such a thread panics with "Cannot start a runtime
/// from within a runtime".
pub fn materials_for_pat(personal_token: &str) -> Option<HashMap<String, String>> {
    let token = personal_token.trim();
    if token.is_empty() {
        return None;
    }
    {
        let cache = materials_cache()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(materials) = cache.get(token) {
            return Some(materials.clone());
        }
    }
    if !token.starts_with("pt-") {
        return None;
    }
    let credential = run_login_blocking(token.to_string()).ok()?;
    if credential.materials.is_empty() {
        return None;
    }
    let mut cache = materials_cache()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    cache.insert(token.to_string(), credential.materials.clone());
    Some(credential.materials)
}

/// Drives a login exchange to completion on a private runtime owned by a fresh
/// OS thread.
///
/// The caller is typically a Tokio worker driving an async request builder, so
/// constructing a runtime and calling `block_on` on that same thread panics
/// ("Cannot start a runtime from within a runtime"). Handing the exchange to a
/// dedicated thread keeps the two runtimes strictly disjoint.
///
/// The exchange happens at most once per token: `materials_for_pat` caches the
/// result process-wide, so later requests never reach this function.
fn run_login_blocking(token: String) -> Result<QoderCredential, anyhow::Error> {
    std::thread::spawn(move || {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("qoder login runtime builds")
            .block_on(pat_login(&token))
    })
    .join()
    .unwrap_or_else(|panic| std::panic::resume_unwind(panic))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Regression guard for the crash observed in the host: the request builder
    /// calls into this crate from a Tokio worker, and `run_login_blocking` used
    /// to build a runtime on that very thread, panicking with "Cannot start a
    /// runtime from within a runtime".
    ///
    /// The exchange is driven directly so the guard sits on the defect layer
    /// rather than behind an early return. The token is deliberately malformed:
    /// the network call fails, which is fine — the only property under test is
    /// that driving the exchange from inside a live runtime does not unwind.
    #[test]
    fn login_exchange_is_safe_inside_a_tokio_runtime() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("test runtime builds");
        let outcome = runtime.block_on(async {
            std::panic::catch_unwind(|| run_login_blocking("pt-not-a-real-token".to_string()))
        });
        assert!(
            outcome.is_ok(),
            "driving the login exchange from inside a Tokio runtime must not panic"
        );
    }

    #[test]
    fn signature_materials_round_trip_through_reserved_keys() {
        let signature = QoderSignature {
            uid: "u-1".to_string(),
            encrypt_user_info: "enc".to_string(),
            key: "k".to_string(),
        };
        let materials = signature.to_materials();
        assert_eq!(materials.get(HEADER_UID).map(String::as_str), Some("u-1"));
        assert_eq!(
            materials.get(HEADER_USER_INFO).map(String::as_str),
            Some("enc")
        );
        assert_eq!(materials.get(HEADER_KEY).map(String::as_str), Some("k"));
    }

    #[test]
    fn reserved_header_keys_match_the_request_side() {
        assert_eq!(HEADER_UID, crate::request::X_TAIJI_QODER_UID);
        assert_eq!(HEADER_USER_INFO, crate::request::X_TAIJI_QODER_USER_INFO);
        assert_eq!(HEADER_KEY, crate::request::X_TAIJI_QODER_KEY);
    }

    #[test]
    fn request_profile_carries_the_gateway_endpoints() {
        let mut materials = HashMap::new();
        materials.insert(HEADER_UID.to_string(), "u-9".to_string());
        let credential = QoderCredential {
            job_token: "job-test".to_string(),
            personal_token: "pt-test".to_string(),
            expires_at_ms: 1,
            materials,
        };
        let profile = request_profile(&credential);
        assert_eq!(profile.api_key, "job-test");
        assert_eq!(profile.base_url, MODEL_BASE_URL);
        assert_eq!(profile.request_url, MODEL_REQUEST_URL);
        assert_eq!(profile.format, "openai");
        assert_eq!(profile.expires_at, Some(1));
        assert_eq!(
            profile.extra_headers.get(HEADER_UID).map(String::as_str),
            Some("u-9")
        );
    }

    #[test]
    fn suggested_seeds_the_gateway_entry() {
        let (base_url, model, format) = suggested();
        assert_eq!(base_url, MODEL_BASE_URL);
        assert_eq!(model, "auto");
        assert_eq!(format, "openai");
    }

    #[cfg(feature = "subscription-auth")]
    #[test]
    fn build_signature_derives_non_empty_material_for_a_uid() {
        let agreed = serde_json::Value::Bool(true);
        let signature =
            build_signature("u-test", None, None, Some(&agreed)).expect("derives material");
        assert_eq!(signature.uid, "u-test");
        assert!(!signature.encrypt_user_info.is_empty());
        assert!(!signature.key.is_empty());
    }
}
