use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use filebridge_transport_api::{ErrorKind, Result, TransportError};
use reqwest::StatusCode;
use serde_json::{Value, json};

use crate::fs::Scrubber;

/// One way to reach rclone's rc API: the `rclone rcd` sidecar over HTTP ([`crate::Rcd`]), or
/// librclone linked into the process (`Librclone`, feature `librclone`, on phones: ADR 0014).
/// Both take the same methods and JSON.
#[async_trait]
pub trait RcCall: Send + Sync {
    /// Calls an rc method. Errors carry a kind from the status and rclone's message
    /// ([`kind_of`]), scrubbed of the secrets in use ([`RcCall::scrubber`]).
    async fn call(&self, method: &str, params: Value) -> Result<Value>;

    /// What scrubs secrets from this rclone's errors and log lines.
    fn scrubber(&self) -> &Scrubber;
}

/// What the transport talks to rclone through. Cheap to clone; keeps rclone (the sidecar, or
/// the library) alive for as long as a clone exists.
#[derive(Clone)]
pub struct RcClient {
    inner: Arc<dyn RcCall>,
}

impl<T: RcCall + 'static> From<Arc<T>> for RcClient {
    fn from(inner: Arc<T>) -> Self {
        Self { inner }
    }
}

impl RcClient {
    /// What scrubs secrets from this rclone's errors and log lines.
    pub fn scrubber(&self) -> &Scrubber {
        self.inner.scrubber()
    }

    /// Calls an rc method.
    pub async fn call(&self, method: &str, params: Value) -> Result<Value> {
        self.inner.call(method, params).await
    }

    /// Starts `method` as an rc job (`_async`) and waits for it to finish.
    pub async fn call_job(&self, method: &str, mut params: Value) -> Result<Value> {
        params["_async"] = json!(true);
        let started = self.call(method, params).await?;
        let jobid = started
            .get("jobid")
            .and_then(Value::as_u64)
            .ok_or_else(|| TransportError::fatal(format!("rclone rc {method}: no jobid")))?;
        loop {
            tokio::time::sleep(Duration::from_millis(200)).await;
            let status = self.call("job/status", json!({ "jobid": jobid })).await?;
            if status.get("finished").and_then(Value::as_bool) != Some(true) {
                continue;
            }
            if status.get("success").and_then(Value::as_bool) == Some(true) {
                return Ok(status);
            }
            let err = status
                .get("error")
                .and_then(Value::as_str)
                .unwrap_or("unknown error");
            return Err(error(
                None,
                &format!("rclone job {method}"),
                &self.scrubber().scrub(err),
            ));
        }
    }
}

/// Authenticated HTTP client for one `rclone rcd` instance.
pub(crate) struct HttpRc {
    http: reqwest::Client,
    base: String,
    user: String,
    pass: String,
    /// Secrets in use with this rcd, scrubbed from every error it returns.
    scrubber: Scrubber,
}

impl HttpRc {
    pub(crate) fn new(port: u16, user: String, pass: String) -> Self {
        let http = reqwest::Client::builder()
            .no_proxy()
            .connect_timeout(Duration::from_secs(5))
            .build()
            .expect("static reqwest config is valid");
        Self {
            http,
            base: format!("http://127.0.0.1:{port}"),
            user,
            pass,
            scrubber: Scrubber::default(),
        }
    }

    pub(crate) fn scrubber(&self) -> &Scrubber {
        &self.scrubber
    }

    /// Calls an rc method. 5xx and network errors are `Transient`.
    pub(crate) async fn call(&self, method: &str, params: Value) -> Result<Value> {
        let resp = self
            .http
            .post(format!("{}/{method}", self.base))
            .basic_auth(&self.user, Some(&self.pass))
            .json(&params)
            .send()
            .await
            .map_err(|e| TransportError::retryable(format!("rclone rc {method}: {e}")))?;
        let status = resp.status();
        let body: Value = resp.json().await.map_err(|e| {
            TransportError::retryable(format!("rclone rc {method}: bad response: {e}"))
        })?;
        answer(method, status, body, &self.scrubber)
    }
}

/// An rc call's result from its status and JSON body, the same for HTTP and librclone. An
/// error's message is scrubbed by `scrubber`.
pub(crate) fn answer(
    method: &str,
    status: StatusCode,
    body: Value,
    scrubber: &Scrubber,
) -> Result<Value> {
    if status.is_success() {
        return Ok(body);
    }
    let msg = body
        .get("error")
        .and_then(Value::as_str)
        .unwrap_or("unknown error");
    Err(error(
        Some(status),
        &format!("rclone rc {method}"),
        &scrubber.scrub(msg),
    ))
}

/// An rclone error, already scrubbed, as a `TransportError`: its kind (see [`kind_of`]) and, for
/// the statuses people can act on, what it means.
fn error(status: Option<StatusCode>, what: &str, message: &str) -> TransportError {
    let hint = match http_status(message) {
        Some(StatusCode::INSUFFICIENT_STORAGE) => "no space left at the destination: ",
        Some(StatusCode::PAYLOAD_TOO_LARGE) => "the server refused a file this large: ",
        Some(StatusCode::FORBIDDEN) => "the server refused this: ",
        _ => "",
    };
    TransportError::new(kind_of(status, message), format!("{what}: {hint}{message}"))
}

/// The HTTP status a backend reported inside rclone's message, as its status token: the code
/// followed by its reason phrase as servers send it (`401 Unauthorized`, `507 Insufficient
/// Storage`). A number alone, or in other words (a size, a file name), is not one.
fn http_status(message: &str) -> Option<StatusCode> {
    let bytes = message.as_bytes();
    let token_at = |i: usize| {
        if i > 0 && bytes[i - 1].is_ascii_alphanumeric() {
            return None;
        }
        let status = StatusCode::from_bytes(message.get(i..i + 3)?.as_bytes()).ok()?;
        let rest = message[i + 3..].strip_prefix(' ')?;
        // Its reason phrase, or the older name some servers still send for 413.
        let reasons = [
            status.canonical_reason(),
            (status == StatusCode::PAYLOAD_TOO_LARGE).then_some("Request Entity Too Large"),
        ];
        reasons.into_iter().flatten().find_map(|reason| {
            let after = rest.strip_prefix(reason)?;
            (!after.starts_with(|c: char| c.is_ascii_alphanumeric())).then_some(status)
        })
    };
    message
        .match_indices(|c: char| c.is_ascii_digit())
        .find_map(|(i, _)| token_at(i))
}

/// How a full destination reads in rclone's messages, lowercased: `ENOSPC` and `EDQUOT` as Go
/// prints them (a local disk, an SFTP server that passes them on), Windows' wording, and the
/// SFTP status codes for both. OpenSSH's server sends a full disk as a bare `SSH_FX_FAILURE`;
/// `core` tells that one apart by measuring the space left.
const NO_SPACE: &[&str] = &[
    "no space left on device",
    "disk quota exceeded",
    "not enough space on the disk",
    "ssh_fx_no_space_on_filesystem",
    "ssh_fx_quota_exceeded",
    // MinIO's drives under their free-space threshold.
    "xminiostoragefull",
];

/// S3's error codes for a key it doesn't know or a secret that doesn't match, lowercased. They
/// come with `StatusCode: 403`, which for S3 is also a bucket the key may not use
/// (`AccessDenied`, left `Other`).
const S3_AUTH: &[&str] = &[
    "api error invalidaccesskeyid",
    "api error signaturedoesnotmatch",
    "api error expiredtoken",
];

/// S3's error codes for a server that is busy or failing for now, lowercased.
const S3_TRANSIENT: &[&str] = &[
    "api error slowdown",
    "api error serviceunavailable",
    "api error internalerror",
    "api error requesttimeout",
];

/// The kind of an rclone error, from the HTTP status of an rc call (`None` for an async job's
/// result) and rclone's message. This is the only place that reads rclone's wording. A backend's
/// HTTP status (WebDAV) is read from its status token, never from free text: `401` is `Auth`;
/// `403` (a blocked name, a share without write access) and `413` are `Other`; `507` (no space,
/// a Nextcloud quota reached) is `Full`; other `5xx` are `Transient`. A full disk elsewhere is
/// read from the system's words for it ([`NO_SPACE`]).
pub(crate) fn kind_of(status: Option<StatusCode>, message: &str) -> ErrorKind {
    if let Some(backend) = http_status(message) {
        match backend {
            StatusCode::UNAUTHORIZED => return ErrorKind::Auth,
            StatusCode::NOT_FOUND => return ErrorKind::NotFound,
            StatusCode::INSUFFICIENT_STORAGE => return ErrorKind::Full,
            StatusCode::FORBIDDEN | StatusCode::PAYLOAD_TOO_LARGE => return ErrorKind::Other,
            StatusCode::TOO_MANY_REQUESTS | StatusCode::LOCKED => return ErrorKind::Transient,
            s if s.is_server_error() => return ErrorKind::Transient,
            _ => {}
        }
    }
    let msg = message.to_ascii_lowercase();
    let has = |needle: &str| msg.contains(needle);
    if NO_SPACE.iter().any(|n| has(n)) {
        ErrorKind::Full
    } else if S3_AUTH.iter().any(|c| has(c)) || has("logon is invalid") || has("logon failure") {
        // S3 refusing the key ("…does not exist in our records" is the key, not a file), or
        // SMB the user (`STATUS_LOGON_FAILURE`).
        ErrorKind::Auth
    } else if has("network name not found") {
        // SMB's `STATUS_BAD_NETWORK_NAME`: no such share. Not a folder a send would create.
        ErrorKind::Other
    } else if status == Some(StatusCode::NOT_FOUND)
        || has("not found")
        || has("no such file")
        || has("does not exist")
    {
        ErrorKind::NotFound
    } else if has("hash unsupported")
        || has("didn't find backend called")
        || has("hash type not supported")
        || has("not supported")
        || has("doesn't support")
    {
        ErrorKind::Unsupported
    } else if matches!(
        status,
        Some(StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN)
    ) || has("unable to authenticate")
        || has("authentication failed")
    {
        // The rc's own status (its credentials), or SSH refusing the sign-in.
        ErrorKind::Auth
    } else if S3_TRANSIENT.iter().any(|c| has(c)) {
        ErrorKind::Transient
    } else if has("api error accessdenied") {
        // S3's 403 for this bucket or object: like WebDAV's, the server refusing this.
        ErrorKind::Other
    } else if status.is_none_or(|s| s.is_server_error()) {
        // An async job's failure without a known cause is most often the network.
        ErrorKind::Transient
    } else {
        ErrorKind::Other
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rclone_errors_get_a_kind() {
        let k = |status: Option<u16>, msg: &str| {
            kind_of(status.map(|s| StatusCode::from_u16(s).unwrap()), msg)
        };
        assert_eq!(k(Some(404), "object not found"), ErrorKind::NotFound);
        assert_eq!(k(Some(500), "directory not found"), ErrorKind::NotFound);
        assert_eq!(
            k(None, "failed to open source object: object not found"),
            ErrorKind::NotFound
        );
        assert_eq!(
            k(Some(500), "hash unsupported: hash type not supported"),
            ErrorKind::Unsupported
        );
        assert_eq!(
            k(None, "ssh: handshake failed: ssh: unable to authenticate"),
            ErrorKind::Auth
        );
        assert_eq!(
            k(
                Some(500),
                "read metadata failed: 401 Unauthorized: 401 Unauthorized"
            ),
            ErrorKind::Auth
        );
        assert_eq!(
            k(Some(500), "connection reset by peer"),
            ErrorKind::Transient
        );
        assert_eq!(k(None, "connection reset by peer"), ErrorKind::Transient);
        assert_eq!(k(Some(400), "bad parameter"), ErrorKind::Other);
    }

    #[test]
    fn s3_and_smb_errors_get_a_kind() {
        let k = |msg: &str| kind_of(Some(StatusCode::INTERNAL_SERVER_ERROR), msg);
        let s3 = |code: &str, text: &str| {
            format!(
                "error in ListJSON: operation error S3: ListObjectsV2, https response error \
                 StatusCode: 403, RequestID: , HostID: , api error {code}: {text}"
            )
        };
        assert_eq!(
            k(&s3(
                "SignatureDoesNotMatch",
                "The request signature we calculated does not match the signature you provided."
            )),
            ErrorKind::Auth
        );
        assert_eq!(
            k(&s3(
                "InvalidAccessKeyId",
                "The AWS Access Key Id you provided does not exist in our records."
            )),
            ErrorKind::Auth
        );
        // A key that may not use this bucket: the server refuses this, not the sign-in.
        assert_eq!(k(&s3("AccessDenied", "Access Denied.")), ErrorKind::Other);
        assert_eq!(
            k(&s3("SlowDown", "Please reduce your request rate.")),
            ErrorKind::Transient
        );
        assert_eq!(
            k(&s3(
                "XMinioStorageFull",
                "Storage backend has reached its minimum free drive threshold."
            )),
            ErrorKind::Full
        );
        assert_eq!(
            k("S3 bucket bucket1 doesn't support about"),
            ErrorKind::Unsupported
        );
        // Phones' librclone has no `s3` backend (ADR 0022).
        assert_eq!(
            k("didn't find backend called \"s3\""),
            ErrorKind::Unsupported
        );
        assert_eq!(
            k(
                "couldn't connect SMB: response error: The attempted logon is invalid. This is \
               either due to a bad username or authentication information."
            ),
            ErrorKind::Auth
        );
        assert_eq!(
            k("Failed to copy: response error: There is not enough space on the disk."),
            ErrorKind::Full
        );
        assert_eq!(
            k(
                "couldn't initialize SMB: response error: {Network Name Not Found} The specified \
               share name cannot be found on the remote server."
            ),
            ErrorKind::Other
        );
    }

    #[test]
    fn a_full_destination_is_full() {
        let k = |msg: &str| kind_of(None, msg);
        // A local disk, and an SFTP server that passes `ENOSPC` / `EDQUOT` on.
        assert_eq!(
            k("Failed to copy: write /mnt/backup/a.mkv: no space left on device"),
            ErrorKind::Full
        );
        assert_eq!(
            k(
                "Failed to copy: Update ReadFrom failed: sftp: \"Disk quota exceeded\" (SSH_FX_FAILURE)"
            ),
            ErrorKind::Full
        );
        assert_eq!(
            k("Failed to copy: sftp: \"No space on filesystem\" (SSH_FX_NO_SPACE_ON_FILESYSTEM)"),
            ErrorKind::Full
        );
        assert_eq!(
            k("Failed to copy: sftp: \"Quota exceeded\" (SSH_FX_QUOTA_EXCEEDED)"),
            ErrorKind::Full
        );
        assert_eq!(
            k("Failed to copy: There is not enough space on the disk."),
            ErrorKind::Full
        );
        // OpenSSH's bare failure says nothing about space: `core` measures it.
        assert_eq!(
            k("Failed to copy: Update ReadFrom failed: sftp: \"Failure\" (SSH_FX_FAILURE)"),
            ErrorKind::Transient
        );
        assert_eq!(
            kind_of(
                Some(StatusCode::INTERNAL_SERVER_ERROR),
                "no space left on device"
            ),
            ErrorKind::Full,
            "the rc's own 500 doesn't hide it"
        );
    }

    #[test]
    fn webdav_statuses_are_read_from_their_token() {
        let k = |msg: &str| kind_of(Some(StatusCode::INTERNAL_SERVER_ERROR), msg);
        assert_eq!(
            k("couldn't list files: 401 Unauthorized: 401 Unauthorized"),
            ErrorKind::Auth
        );
        // Forbidden is the server refusing this file or folder, not a wrong password.
        assert_eq!(
            k("failed to upload: 403 Forbidden: Sabre\\DAV\\Exception\\Forbidden"),
            ErrorKind::Other
        );
        assert_eq!(k("upload failed: 413 Payload Too Large"), ErrorKind::Other);
        assert_eq!(
            k("upload failed: 413 Request Entity Too Large"),
            ErrorKind::Other
        );
        assert_eq!(
            k("Update failed: 507 Insufficient Storage: quota exceeded"),
            ErrorKind::Full
        );
        assert_eq!(
            k(
                "failed to upload: 507 Insufficient Storage: Sabre\\DAV\\Exception\\InsufficientStorage"
            ),
            ErrorKind::Full
        );
        assert_eq!(k("PUT: 503 Service Unavailable"), ErrorKind::Transient);
        assert_eq!(k("too fast: 429 Too Many Requests"), ErrorKind::Transient);
        // "401" in free text is a name or a number, not a status.
        assert_eq!(
            k("copy room-401 unauthorized-copies/x: connection reset"),
            ErrorKind::Transient
        );
        assert_eq!(
            k("file 401.txt: 4010 Unauthorized bytes"),
            ErrorKind::Transient
        );
        let e = error(None, "rclone job x", "507 Insufficient Storage");
        assert!(e.message.contains("no space left"), "{}", e.message);
    }
}
