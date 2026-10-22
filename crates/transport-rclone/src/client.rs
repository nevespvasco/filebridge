use std::time::Duration;

use filebridge_transport_api::{ErrorKind, Result, TransportError};
use reqwest::StatusCode;
use serde_json::{Value, json};

use crate::fs::Scrubber;

/// Authenticated client for one `rclone rcd` instance.
#[derive(Clone)]
pub struct RcClient {
    http: reqwest::Client,
    base: String,
    user: String,
    pass: String,
    /// Secrets in use with this rcd, scrubbed from every error it returns.
    scrubber: Scrubber,
}

impl RcClient {
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

    /// What scrubs secrets from this rcd's errors and log lines.
    pub fn scrubber(&self) -> &Scrubber {
        &self.scrubber
    }

    /// Calls an rc method. Errors get a kind from the HTTP status and rclone's message (see
    /// [`kind_of`]): 5xx and network errors are `Transient`.
    pub async fn call(&self, method: &str, params: Value) -> Result<Value> {
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
            &self.scrubber.scrub(msg),
        ))
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
                &self.scrubber.scrub(err),
            ));
        }
    }
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

/// The kind of an rclone error, from the HTTP status of an rc call (`None` for an async job's
/// result) and rclone's message. This is the only place that reads rclone's wording. A backend's
/// HTTP status (WebDAV) is read from its status token, never from free text: `401` is `Auth`;
/// `403` (a blocked name, a share without write access) and `413` are `Other`; `507` (no space)
/// is `Other` too, since trying again won't free space; other `5xx` are `Transient`.
pub(crate) fn kind_of(status: Option<StatusCode>, message: &str) -> ErrorKind {
    if let Some(backend) = http_status(message) {
        match backend {
            StatusCode::UNAUTHORIZED => return ErrorKind::Auth,
            StatusCode::NOT_FOUND => return ErrorKind::NotFound,
            StatusCode::FORBIDDEN
            | StatusCode::PAYLOAD_TOO_LARGE
            | StatusCode::INSUFFICIENT_STORAGE => return ErrorKind::Other,
            StatusCode::TOO_MANY_REQUESTS | StatusCode::LOCKED => return ErrorKind::Transient,
            s if s.is_server_error() => return ErrorKind::Transient,
            _ => {}
        }
    }
    let msg = message.to_ascii_lowercase();
    let has = |needle: &str| msg.contains(needle);
    if status == Some(StatusCode::NOT_FOUND)
        || has("not found")
        || has("no such file")
        || has("does not exist")
    {
        ErrorKind::NotFound
    } else if has("hash unsupported") || has("hash type not supported") || has("not supported") {
        ErrorKind::Unsupported
    } else if matches!(
        status,
        Some(StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN)
    ) || has("unable to authenticate")
        || has("authentication failed")
    {
        // The rc's own status (its credentials), or SSH refusing the sign-in.
        ErrorKind::Auth
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
            ErrorKind::Other
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
