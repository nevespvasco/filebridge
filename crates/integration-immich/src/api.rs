//! The few calls of Immich's REST API this integration makes (`<server>/api/…`, the key in the
//! `x-api-key` header). Library calls need an administrator's key.

use std::time::Duration;

use filebridge_integration_api::IntegrationError;
use reqwest::{Method, StatusCode};
use serde::Deserialize;
use serde_json::Value;

/// What went wrong with a call, as far as filebridge acts on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApiErrorKind {
    /// No answer: the server is down, the address is wrong, the network is gone.
    Unreachable,
    /// `401`: the API key is wrong, or was deleted.
    Auth,
    /// `403`: the key's user is not an administrator, or the key lacks the permission.
    Forbidden,
    /// `400`/`404` on a library: no library with that id.
    NotFound,
    /// `429` or `5xx`: Immich is busy or restarting; trying again later may work.
    Busy,
    /// Anything else, e.g. an answer that isn't Immich's.
    Other,
}

/// An API error. Its message never holds the key (it only travels in a header).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiError {
    pub kind: ApiErrorKind,
    pub message: String,
}

impl ApiError {
    fn new(kind: ApiErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    pub fn is_retryable(&self) -> bool {
        matches!(self.kind, ApiErrorKind::Unreachable | ApiErrorKind::Busy)
    }
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl From<ApiError> for IntegrationError {
    fn from(e: ApiError) -> Self {
        if e.is_retryable() {
            IntegrationError::retryable(e.message)
        } else {
            IntegrationError::fatal(e.message)
        }
    }
}

pub type ApiResult<T> = std::result::Result<T, ApiError>;

/// An external library, as `GET /api/libraries/{id}` answers.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Library {
    pub id: String,
    pub name: String,
    /// Folders Immich imports from, as Immich sees them.
    #[serde(default)]
    pub import_paths: Vec<String>,
    #[serde(default)]
    pub exclusion_patterns: Vec<String>,
    #[serde(default)]
    pub asset_count: u64,
}

/// One import path as `POST /api/libraries/{id}/validate` judges it: whether Immich can read it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PathValidation {
    pub import_path: String,
    pub is_valid: bool,
    #[serde(default)]
    pub message: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Validation {
    #[serde(default)]
    import_paths: Vec<PathValidation>,
}

#[derive(Deserialize)]
struct Version {
    major: u32,
    minor: u32,
    patch: u32,
}

/// Immich's error text from an error answer (`{"message": "…"}` or a list of them).
fn immich_message(body: &[u8]) -> Option<String> {
    let v: Value = serde_json::from_slice(body).ok()?;
    match v.get("message")? {
        Value::String(s) => Some(s.clone()),
        Value::Array(a) => Some(
            a.iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join("; "),
        ),
        _ => None,
    }
}

/// An HTTP client for Immich servers. One per integration; every call names its server.
#[derive(Debug, Clone)]
pub struct Api {
    http: reqwest::Client,
}

impl Default for Api {
    fn default() -> Self {
        Self::new()
    }
}

impl Api {
    pub fn new() -> Self {
        let builder = || {
            reqwest::Client::builder()
                .connect_timeout(Duration::from_secs(15))
                .timeout(Duration::from_secs(60))
        };
        // The OS's certificates and the bundled Mozilla roots; without the OS's when its store
        // can't be read, rather than no client at all.
        let http = builder()
            .build()
            .or_else(|_| builder().tls_built_in_native_certs(false).build())
            .expect("a client with the bundled roots always builds");
        Self { http }
    }

    async fn call(
        &self,
        method: Method,
        server: &str,
        path: &str,
        key: Option<&str>,
        body: Option<Value>,
    ) -> ApiResult<Vec<u8>> {
        let mut req = self
            .http
            .request(method, format!("{server}/api{path}"))
            .header(reqwest::header::ACCEPT, "application/json");
        if let Some(key) = key {
            req = req.header("x-api-key", key);
        }
        if let Some(body) = body {
            req = req.json(&body);
        }
        let res = req.send().await.map_err(|e| {
            ApiError::new(
                ApiErrorKind::Unreachable,
                format!("could not reach Immich at {server}: {}", e.without_url()),
            )
        })?;
        let status = res.status();
        let bytes = res.bytes().await.map_err(|e| {
            ApiError::new(
                ApiErrorKind::Unreachable,
                format!("Immich at {server} stopped answering: {}", e.without_url()),
            )
        })?;
        if status.is_success() {
            return Ok(bytes.to_vec());
        }
        let said = immich_message(&bytes)
            .map(|m| format!(": {m}"))
            .unwrap_or_default();
        let kind = match status {
            StatusCode::UNAUTHORIZED => ApiErrorKind::Auth,
            StatusCode::FORBIDDEN => ApiErrorKind::Forbidden,
            StatusCode::NOT_FOUND | StatusCode::BAD_REQUEST => ApiErrorKind::NotFound,
            StatusCode::TOO_MANY_REQUESTS => ApiErrorKind::Busy,
            s if s.is_server_error() => ApiErrorKind::Busy,
            _ => ApiErrorKind::Other,
        };
        Err(ApiError::new(
            kind,
            format!("Immich answered {status}{said}"),
        ))
    }

    fn decode<T: serde::de::DeserializeOwned>(server: &str, body: &[u8]) -> ApiResult<T> {
        serde_json::from_slice(body).map_err(|e| {
            ApiError::new(
                ApiErrorKind::Other,
                format!("{server} did not answer like Immich ({e})"),
            )
        })
    }

    /// `GET /api/server/ping` (no key): whether an Immich answers at `server`.
    pub async fn ping(&self, server: &str) -> ApiResult<()> {
        let body = self
            .call(Method::GET, server, "/server/ping", None, None)
            .await
            .map_err(|e| match e.kind {
                ApiErrorKind::NotFound | ApiErrorKind::Auth | ApiErrorKind::Forbidden => {
                    ApiError::new(
                        ApiErrorKind::Other,
                        format!("{server} is not an Immich server ({})", e.message),
                    )
                }
                _ => e,
            })?;
        let v: Value = Self::decode(server, &body)?;
        if v.get("res").and_then(Value::as_str) == Some("pong") {
            Ok(())
        } else {
            Err(ApiError::new(
                ApiErrorKind::Other,
                format!("{server} is not an Immich server"),
            ))
        }
    }

    /// `GET /api/server/version` (no key), as `v1.2.3`.
    pub async fn version(&self, server: &str) -> ApiResult<String> {
        let body = self
            .call(Method::GET, server, "/server/version", None, None)
            .await?;
        let v: Version = Self::decode(server, &body)?;
        Ok(format!("v{}.{}.{}", v.major, v.minor, v.patch))
    }

    /// `GET /api/libraries`: every external library (administrators only).
    pub async fn libraries(&self, server: &str, key: &str) -> ApiResult<Vec<Library>> {
        let body = self
            .call(Method::GET, server, "/libraries", Some(key), None)
            .await?;
        Self::decode(server, &body)
    }

    /// `GET /api/libraries/{id}`. `id` must be a UUID ([`crate::valid_library_id`]).
    pub async fn library(&self, server: &str, key: &str, id: &str) -> ApiResult<Library> {
        let body = self
            .call(
                Method::GET,
                server,
                &format!("/libraries/{id}"),
                Some(key),
                None,
            )
            .await?;
        Self::decode(server, &body)
    }

    /// `POST /api/libraries/{id}/validate`: whether Immich can read each of `paths`.
    pub async fn validate(
        &self,
        server: &str,
        key: &str,
        id: &str,
        paths: &[String],
    ) -> ApiResult<Vec<PathValidation>> {
        let body = self
            .call(
                Method::POST,
                server,
                &format!("/libraries/{id}/validate"),
                Some(key),
                Some(serde_json::json!({ "importPaths": paths })),
            )
            .await?;
        Ok(Self::decode::<Validation>(server, &body)?.import_paths)
    }

    /// `POST /api/libraries/{id}/scan`: queues a scan of the whole library (Immich has no scan
    /// by path). New and changed files are imported, missing ones marked offline. Returns
    /// once queued; Immich scans in the background.
    pub async fn scan(&self, server: &str, key: &str, id: &str) -> ApiResult<()> {
        // Older Immich versions read optional flags from the body; `{}` keeps their defaults
        // (new and changed files only), and later versions take no body.
        self.call(
            Method::POST,
            server,
            &format!("/libraries/{id}/scan"),
            Some(key),
            Some(serde_json::json!({})),
        )
        .await
        .map(|_| ())
    }
}
