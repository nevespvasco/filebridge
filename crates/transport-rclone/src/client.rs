use std::time::Duration;

use filebridge_transport_api::{Result, TransportError};
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

    /// Calls an rc method. 5xx errors are reported as retryable, 4xx as fatal.
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
        Err(TransportError {
            message: format!("rclone rc {method}: {msg}"),
            retryable: status.is_server_error(),
        })
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
            return Err(TransportError::retryable(format!(
                "rclone job {method}: {err}"
            )));
        }
    }
}
