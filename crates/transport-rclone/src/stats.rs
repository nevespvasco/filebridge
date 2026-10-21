//! `core/stats` polling, forwarded to a `ProgressSink`.

use std::time::Duration;

use filebridge_transport_api::{ProgressEvent, ProgressSink, Result, StatsSnapshot};
use serde_json::{Value, json};
use tokio::task::JoinHandle;

use crate::RcClient;

/// Reads the stats of one rc stats group.
pub async fn snapshot(client: &RcClient, group: &str) -> Result<StatsSnapshot> {
    let v = client.call("core/stats", json!({ "group": group })).await?;
    let u = |k: &str| v.get(k).and_then(Value::as_u64).unwrap_or(0);
    let f = |k: &str| v.get(k).and_then(Value::as_f64).unwrap_or(0.0);
    Ok(StatsSnapshot {
        bytes: u("bytes"),
        total_bytes: u("totalBytes"),
        files_done: u("transfers"),
        total_files: u("totalTransfers"),
        bytes_per_sec: f("speed"),
        errors: u("errors"),
        elapsed_secs: f("elapsedTime"),
    })
}

/// Polls `group` every `every` and sends a `Stats` event; abort the handle to stop.
/// `totals` overrides rclone's running totals when the caller knows the real ones.
pub fn spawn_poller(
    client: RcClient,
    group: String,
    totals: Option<(u64, u64)>,
    sink: ProgressSink,
    every: Duration,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(every);
        loop {
            tick.tick().await;
            if let Ok(mut s) = snapshot(&client, &group).await {
                if let Some((files, bytes)) = totals {
                    s.total_files = files;
                    s.total_bytes = bytes;
                }
                sink.send(ProgressEvent::Stats(s));
            }
        }
    })
}
