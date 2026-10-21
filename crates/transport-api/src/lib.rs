//! The `Transport` contract: move bytes from a source to a destination, with progress, resume
//! and hashing. No implementation code lives here.
//!
//! `specta::Type` derives are added in Phase 1, when these types first cross into the UI.

use std::fmt;
use std::path::PathBuf;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

pub type Result<T, E = TransportError> = std::result::Result<T, E>;

/// Every transport error says whether retrying could help, so `core` picks backoff or failure.
#[derive(Debug, Clone, thiserror::Error, Serialize, Deserialize)]
#[error("{message}")]
pub struct TransportError {
    pub message: String,
    pub retryable: bool,
}

impl TransportError {
    pub fn retryable(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            retryable: true,
        }
    }

    pub fn fatal(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            retryable: false,
        }
    }
}

bitflags::bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
    pub struct TransportCaps: u32 {
        const RESUME = 1 << 0;
        const HASH_SHA256 = 1 << 1;
        const HASH_XXH = 1 << 2;
        const CHOWN = 1 << 3;
        const CHMOD = 1 << 4;
        const RENAME = 1 << 5;
        const PARTIAL_HIDDEN = 1 << 6;
    }
}

/// Reference to a secret stored in the OS keyring. Never the secret itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyringRef(pub String);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Backend {
    Local,
    Sftp {
        host: String,
        port: Option<u16>,
        user: Option<String>,
        /// Private key file; when absent the SSH agent is used.
        key_file: Option<PathBuf>,
        /// Host keys are always verified; defaults to `~/.ssh/known_hosts`.
        known_hosts_file: Option<PathBuf>,
        password: Option<KeyringRef>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Destination {
    pub transport: String,
    pub backend: Backend,
    /// Root directory on the destination; every `RemotePath` is relative to it.
    pub root: String,
}

/// Destination-relative path, `/`-separated. Producers (the `names` rules) guarantee it is NFC.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct RemotePath(String);

impl RemotePath {
    /// Rejects absolute paths, empty components, `.` and `..`.
    pub fn new(path: impl Into<String>) -> Result<Self> {
        let path = path.into();
        let bad = path.is_empty()
            || path.starts_with('/')
            || path
                .split('/')
                .any(|c| c.is_empty() || c == "." || c == "..");
        if bad {
            return Err(TransportError::fatal(format!(
                "invalid remote path: {path:?}"
            )));
        }
        Ok(Self(path))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Parent directory, `None` at the root.
    pub fn parent(&self) -> Option<&str> {
        self.0.rsplit_once('/').map(|(p, _)| p)
    }

    pub fn file_name(&self) -> &str {
        self.0.rsplit_once('/').map_or(&self.0, |(_, n)| n)
    }
}

impl TryFrom<String> for RemotePath {
    type Error = TransportError;
    fn try_from(s: String) -> Result<Self> {
        Self::new(s)
    }
}

impl From<RemotePath> for String {
    fn from(p: RemotePath) -> Self {
        p.0
    }
}

impl fmt::Display for RemotePath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CheckStatus {
    Ok,
    Warn,
    Fail,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Check {
    pub id: String,
    pub status: CheckStatus,
    pub message: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PreflightReport {
    pub free_bytes: Option<u64>,
    pub writable: bool,
    pub latency_ms: Option<u64>,
    pub checks: Vec<Check>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum HashAlgo {
    Sha256,
    Xxh3_128,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Digest {
    pub algo: HashAlgo,
    pub bytes: Vec<u8>,
}

impl Digest {
    pub fn hex(&self) -> String {
        self.bytes.iter().map(|b| format!("{b:02x}")).collect()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransferItem {
    /// Relative to `TransferJob::source_root`; may hold non-UTF-8 bytes.
    pub source: PathBuf,
    /// Already-corrected destination name.
    pub dest: RemotePath,
    pub size: u64,
    pub expected_hash: Option<Digest>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransferOptions {
    /// Files in flight at once.
    pub concurrency: usize,
    /// rclone-style limit, e.g. `"20M"`.
    pub bwlimit: Option<String>,
}

impl Default for TransferOptions {
    fn default() -> Self {
        Self {
            concurrency: 4,
            bwlimit: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransferJob {
    pub id: String,
    pub source_root: PathBuf,
    pub items: Vec<TransferItem>,
    pub options: TransferOptions,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ItemOutcome {
    Done,
    Failed(TransportError),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransferResult {
    /// Same order as `TransferJob::items`.
    pub items: Vec<ItemOutcome>,
    pub bytes: u64,
}

impl TransferResult {
    pub fn failed(&self) -> usize {
        self.items
            .iter()
            .filter(|o| matches!(o, ItemOutcome::Failed(_)))
            .count()
    }
}

/// Point-in-time transfer statistics.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StatsSnapshot {
    pub bytes: u64,
    pub total_bytes: u64,
    pub files_done: u64,
    pub total_files: u64,
    pub bytes_per_sec: f64,
    pub errors: u64,
    pub elapsed_secs: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ProgressEvent {
    /// A named stage started (e.g. "copy", "rename", "move-into-place").
    Stage(String),
    Stats(StatsSnapshot),
    ItemDone {
        index: usize,
        ok: bool,
    },
}

/// Cloneable, cheap-to-call handle for progress events. Dropped events are fine: progress is
/// advisory, the result is authoritative.
#[derive(Debug, Clone, Default)]
pub struct ProgressSink(Option<mpsc::UnboundedSender<ProgressEvent>>);

impl ProgressSink {
    pub fn channel() -> (Self, mpsc::UnboundedReceiver<ProgressEvent>) {
        let (tx, rx) = mpsc::unbounded_channel();
        (Self(Some(tx)), rx)
    }

    /// A sink that discards everything.
    pub fn noop() -> Self {
        Self(None)
    }

    pub fn send(&self, event: ProgressEvent) {
        if let Some(tx) = &self.0 {
            let _ = tx.send(event);
        }
    }
}

#[async_trait]
pub trait Transport: Send + Sync {
    fn id(&self) -> &str;
    fn capabilities(&self) -> TransportCaps;
    /// Space, permissions, connectivity.
    async fn preflight(&self, dest: &Destination) -> Result<PreflightReport>;
    async fn transfer(
        &self,
        dest: &Destination,
        job: &TransferJob,
        progress: ProgressSink,
    ) -> Result<TransferResult>;
    async fn hash(&self, dest: &Destination, path: &RemotePath, algo: HashAlgo) -> Result<Digest>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remote_path_validation() {
        assert!(RemotePath::new("a/b.txt").is_ok());
        for bad in ["", "/abs", "a//b", "a/./b", "../x", "a/"] {
            assert!(RemotePath::new(bad).is_err(), "{bad:?} should be rejected");
        }
        let p = RemotePath::new("Fotos/Verão/praia.jpg").unwrap();
        assert_eq!(p.parent(), Some("Fotos/Verão"));
        assert_eq!(p.file_name(), "praia.jpg");
        assert_eq!(RemotePath::new("top").unwrap().parent(), None);
    }

    #[test]
    fn digest_hex() {
        let d = Digest {
            algo: HashAlgo::Sha256,
            bytes: vec![0x00, 0xab, 0xff],
        };
        assert_eq!(d.hex(), "00abff");
    }
}
