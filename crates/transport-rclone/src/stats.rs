//! `core/stats` polling, forwarded to a `ProgressSink`.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use filebridge_transport_api::{
    InFlight, ProgressEvent, ProgressSink, Result, StatsSnapshot, TransferJob,
};
use serde_json::{Value, json};
use tokio::task::JoinHandle;

use crate::RcClient;

/// How rclone names a file in flight → the item's destination name. For `operations/copyfile`
/// rclone reports the **source** path (checked with rclone v1.75.2: copying `src-name.bin` to
/// `dest-name.bin` shows `src-name.bin`); the destination name is mapped too, so a strategy or an
/// rclone version that reports it still works.
#[derive(Debug, Default)]
pub struct InFlightNames(HashMap<String, String>);

impl InFlightNames {
    pub fn of(job: &TransferJob) -> Self {
        let mut names = HashMap::with_capacity(job.items.len() * 2);
        for item in &job.items {
            let dest = item.dest.as_str().to_owned();
            let source = item.source.to_string_lossy().replace('\\', "/");
            names.insert(source, dest.clone());
            names.insert(dest.clone(), dest);
        }
        Self(names)
    }

    fn dest_of(&self, name: &str) -> Option<&str> {
        self.0.get(name).map(String::as_str)
    }
}

/// Reads the stats of one rc stats group. With `names`, files in flight are reported under their
/// destination names, and files rclone moves that aren't items of the job are left out.
pub async fn snapshot(
    client: &RcClient,
    group: &str,
    names: Option<&InFlightNames>,
) -> Result<StatsSnapshot> {
    let v = client.call("core/stats", json!({ "group": group })).await?;
    Ok(parse_stats(&v, names))
}

fn parse_stats(v: &Value, names: Option<&InFlightNames>) -> StatsSnapshot {
    let u = |k: &str| v.get(k).and_then(Value::as_u64).unwrap_or(0);
    let f = |k: &str| v.get(k).and_then(Value::as_f64).unwrap_or(0.0);
    let transferring = v
        .get("transferring")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|t| {
            let name = t.get("name").and_then(Value::as_str)?;
            let dest = match names {
                Some(n) => n.dest_of(name)?.to_owned(),
                None => name.to_owned(),
            };
            Some(InFlight {
                dest,
                size: t.get("size").and_then(Value::as_u64).unwrap_or(0),
                bytes: t.get("bytes").and_then(Value::as_u64).unwrap_or(0),
                bytes_per_sec: t.get("speed").and_then(Value::as_f64).unwrap_or(0.0),
                eta_secs: t.get("eta").and_then(Value::as_u64),
            })
        })
        .collect();
    StatsSnapshot {
        bytes: u("bytes"),
        total_bytes: u("totalBytes"),
        files_done: u("transfers"),
        total_files: u("totalTransfers"),
        bytes_per_sec: f("speed"),
        errors: u("errors"),
        elapsed_secs: f("elapsedTime"),
        transferring,
    }
}

/// Polls `group` every `every` and sends a `Stats` event; abort the handle to stop.
/// `totals` overrides rclone's running totals when the caller knows the real ones.
pub fn spawn_poller(
    client: RcClient,
    group: String,
    totals: Option<(u64, u64)>,
    names: Option<Arc<InFlightNames>>,
    sink: ProgressSink,
    every: Duration,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(every);
        loop {
            tick.tick().await;
            if let Ok(mut s) = snapshot(&client, &group, names.as_deref()).await {
                if let Some((files, bytes)) = totals {
                    s.total_files = files;
                    s.total_bytes = bytes;
                }
                sink.send(ProgressEvent::Stats(s));
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use filebridge_transport_api::{RemotePath, TransferItem, TransferOptions};

    use super::*;

    fn job() -> TransferJob {
        TransferJob {
            id: "j".into(),
            source_root: PathBuf::from("/src"),
            items: vec![TransferItem {
                source: PathBuf::from("Fotos/Vera\u{303}o.jpg"),
                dest: RemotePath::new("Fotos/Ver\u{e3}o.jpg").unwrap(),
                size: 10,
                expected_hash: None,
            }],
            options: TransferOptions::default(),
        }
    }

    #[test]
    fn files_in_flight_get_their_destination_names() {
        let v = json!({
            "bytes": 5, "speed": 2.0,
            "transferring": [
                { "name": "Fotos/Vera\u{303}o.jpg", "size": 10, "bytes": 4, "speed": 2.0, "eta": 3 },
                { "name": "Fotos/Ver\u{e3}o.jpg", "size": 10, "bytes": 5, "speed": 1.5, "eta": null },
                { "name": "not-in-the-job.tmp", "size": 1, "bytes": 0 }
            ]
        });
        let names = InFlightNames::of(&job());
        let s = parse_stats(&v, Some(&names));
        assert_eq!(s.transferring.len(), 2, "unknown names are left out");
        assert!(
            s.transferring
                .iter()
                .all(|f| f.dest == "Fotos/Ver\u{e3}o.jpg")
        );
        assert_eq!(s.transferring[0].eta_secs, Some(3));
        assert_eq!(s.transferring[1].eta_secs, None);
        // Without names (the bench), rclone's names are kept as they are.
        assert_eq!(parse_stats(&v, None).transferring.len(), 3);
    }

    #[test]
    fn no_transferring_list_is_an_empty_one() {
        assert!(
            parse_stats(&json!({ "bytes": 1 }), None)
                .transferring
                .is_empty()
        );
    }
}
