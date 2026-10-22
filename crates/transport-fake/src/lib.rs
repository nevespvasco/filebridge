//! A `Transport` adapter that copies files into a local directory (`Backend::Local`, the
//! destination's `root`), with knobs to make it fail the way real destinations do. It keeps the
//! same contract as rclone (`filebridge_transport_api::contract`), so tests of `core`, `ipc` and
//! the service run against it instead of each keeping its own fake.

use std::collections::HashSet;
use std::io::ErrorKind as IoKind;
use std::path::Path;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use filebridge_transport_api::{
    Destination, Digest, HashAlgo, InFlight, ItemOutcome, PreflightReport, ProgressEvent,
    ProgressSink, RemoteEntry, RemotePath, Result, StatsSnapshot, TransferJob, TransferResult,
    Transport, TransportCaps, TransportError,
};
use sha2::{Digest as _, Sha256};

#[derive(Default)]
pub struct FakeTransport {
    /// Whole transfer calls that fail with a retryable error before one succeeds.
    pub fail_calls: AtomicU32,
    /// Destination file names that always fail with a fatal error.
    pub fatal_names: Mutex<HashSet<String>>,
    /// Destination file names written corrupted the first time.
    pub corrupt_once: Mutex<HashSet<String>>,
    /// Destination file names reported sent the first time but never written: gone by the time
    /// they are verified.
    pub lose_once: Mutex<HashSet<String>>,
    /// Delay per file, to make pausing observable.
    pub per_file: Mutex<Duration>,
    /// Transfer calls so far.
    pub calls: AtomicU32,
    /// The destination can't compute hashes.
    pub no_hash: AtomicBool,
    /// Every bandwidth limit set, in order.
    pub bandwidth_limits: Mutex<Vec<Option<u64>>>,
}

impl FakeTransport {
    pub const ID: &'static str = "fake";
}

fn io_error(context: &str, e: std::io::Error) -> TransportError {
    let message = format!("{context}: {e}");
    match e.kind() {
        IoKind::NotFound => TransportError::not_found(message),
        _ => TransportError::fatal(message),
    }
}

#[async_trait]
impl Transport for FakeTransport {
    fn id(&self) -> &str {
        Self::ID
    }

    fn capabilities(&self) -> TransportCaps {
        TransportCaps::RESUME
            | TransportCaps::HASH_SHA256
            | TransportCaps::RENAME
            | TransportCaps::LIST
            | TransportCaps::MOVE
    }

    async fn preflight(&self, _dest: &Destination) -> Result<PreflightReport> {
        Ok(PreflightReport {
            free_bytes: Some(1 << 40),
            total_bytes: Some(3 << 40),
            used_bytes: Some(2 << 40),
            writable: true,
            ..Default::default()
        })
    }

    async fn transfer(
        &self,
        dest: &Destination,
        job: &TransferJob,
        progress: ProgressSink,
    ) -> Result<TransferResult> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if self
            .fail_calls
            .try_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1))
            .is_ok()
        {
            return Err(TransportError::retryable("connection reset by peer"));
        }
        let delay = *self.per_file.lock().unwrap();
        let mut items = Vec::with_capacity(job.items.len());
        let mut bytes = 0;
        for (i, item) in job.items.iter().enumerate() {
            // One file at a time: it is the one in flight until it is copied.
            progress.send(ProgressEvent::Stats(StatsSnapshot {
                bytes,
                files_done: i as u64,
                bytes_per_sec: 1000.0,
                transferring: vec![InFlight {
                    dest: item.dest.as_str().to_owned(),
                    size: item.size,
                    ..Default::default()
                }],
                ..Default::default()
            }));
            if !delay.is_zero() {
                tokio::time::sleep(delay).await;
            }
            let name = item.dest.file_name().to_owned();
            if self.fatal_names.lock().unwrap().contains(&name) {
                items.push(ItemOutcome::Failed(TransportError::fatal(
                    "permission denied",
                )));
                continue;
            }
            let from = job.source_root.join(&item.source);
            let to = Path::new(&dest.root).join(item.dest.as_str());
            let copied = std::fs::create_dir_all(to.parent().expect("dest has a parent"))
                .map_err(|e| io_error("create directory", e))
                .and_then(|()| {
                    if self.lose_once.lock().unwrap().remove(&name) {
                        Ok(())
                    } else if self.corrupt_once.lock().unwrap().remove(&name) {
                        std::fs::metadata(&from).map_err(|e| io_error("read source", e))?;
                        std::fs::write(&to, b"garbage").map_err(|e| io_error("write", e))
                    } else {
                        std::fs::copy(&from, &to)
                            .map(drop)
                            .map_err(|e| io_error("copy", e))
                    }
                });
            if let Err(e) = copied {
                progress.send(ProgressEvent::ItemDone {
                    index: i,
                    ok: false,
                });
                items.push(ItemOutcome::Failed(e));
                continue;
            }
            bytes += item.size;
            progress.send(ProgressEvent::ItemDone { index: i, ok: true });
            progress.send(ProgressEvent::Stats(StatsSnapshot {
                bytes,
                files_done: i as u64 + 1,
                bytes_per_sec: 1000.0,
                ..Default::default()
            }));
            items.push(ItemOutcome::Done);
        }
        Ok(TransferResult { items, bytes })
    }

    async fn hash(&self, dest: &Destination, path: &RemotePath, algo: HashAlgo) -> Result<Digest> {
        if self.no_hash.load(Ordering::SeqCst) || algo != HashAlgo::Sha256 {
            return Err(TransportError::unsupported(format!(
                "this destination can't compute {algo:?}"
            )));
        }
        let data = std::fs::read(Path::new(&dest.root).join(path.as_str()))
            .map_err(|e| io_error(path.as_str(), e))?;
        Ok(Digest {
            algo,
            bytes: Sha256::digest(&data).to_vec(),
        })
    }

    async fn move_path(
        &self,
        dest: &Destination,
        from: &RemotePath,
        to: &RemotePath,
    ) -> Result<()> {
        let root = Path::new(&dest.root);
        let src = root.join(from.as_str());
        std::fs::symlink_metadata(&src).map_err(|e| io_error("move", e))?;
        if from == to {
            return Ok(());
        }
        let parent = root.join(to.parent().unwrap_or(""));
        std::fs::create_dir_all(&parent).map_err(|e| io_error("move", e))?;
        // Exact names only: on a filesystem that normalises or ignores case, `to` may be
        // `from` itself under another spelling, and that is not a clash.
        let taken = std::fs::read_dir(&parent)
            .map_err(|e| io_error("move", e))?
            .filter_map(|e| e.ok())
            .any(|e| e.file_name().to_str() == Some(to.file_name()));
        if taken {
            return Err(TransportError::fatal(format!("{to} already exists")));
        }
        let mid = parent.join(format!(".fb-move-{}", std::process::id()));
        std::fs::rename(&src, &mid).map_err(|e| io_error("move", e))?;
        std::fs::rename(&mid, root.join(to.as_str())).map_err(|e| io_error("move", e))
    }

    async fn list(
        &self,
        dest: &Destination,
        dir: Option<&RemotePath>,
        dirs_only: bool,
    ) -> Result<Vec<RemoteEntry>> {
        let path = match dir {
            Some(d) => Path::new(&dest.root).join(d.as_str()),
            None => Path::new(&dest.root).to_path_buf(),
        };
        let read = std::fs::read_dir(&path).map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => {
                TransportError::not_found(format!("{}: {e}", path.display()))
            }
            _ => io_error("list", e),
        })?;
        let mut out = Vec::new();
        for item in read {
            let item = item.map_err(|e| io_error("list", e))?;
            let meta = item.metadata().map_err(|e| io_error("list", e))?;
            if dirs_only && !meta.is_dir() {
                continue;
            }
            out.push(RemoteEntry {
                name: item.file_name().to_string_lossy().into_owned(),
                is_dir: meta.is_dir(),
                size: meta.is_file().then_some(meta.len()),
                mtime: meta
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_millis() as i64),
            });
        }
        out.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(out)
    }

    async fn set_bandwidth_limit(&self, bytes_per_sec: Option<u64>) -> Result<()> {
        self.bandwidth_limits.lock().unwrap().push(bytes_per_sec);
        Ok(())
    }
}
