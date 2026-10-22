use std::time::Duration;

use filebridge_transport_api::{ErrorKind, Result, TransportError};
use reqwest::StatusCode;
use serde_json::{Value, json};

/// Authenticated client for one `rclone rcd` instance.
#[derive(Clone)]
pub struct RcClient {
    http: reqwest::Client,
    base: String,
    user: String,
    pass: String,
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
        }
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
        Err(TransportError::new(
            kind_of(Some(status), msg),
            format!("rclone rc {method}: {}", crate::fs::redact(msg)),
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
            return Err(TransportError::new(
                kind_of(None, err),
                format!("rclone job {method}: {}", crate::fs::redact(err)),
            ));
        }
    }
}

/// The kind of an rclone error, from the HTTP status of an rc call (`None` for an async job's
/// result) and rclone's message. This is the only place that reads rclone's wording.
pub(crate) fn kind_of(status: Option<StatusCode>, message: &str) -> ErrorKind {
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
        // WebDAV servers (Nextcloud) answer a wrong password with HTTP 401, which rclone
        // repeats inside its own 500 answer.
        || has("401 unauthorized")
    {
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
}
