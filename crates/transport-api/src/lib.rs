//! The `Transport` contract: move bytes from a source to a destination, with progress, resume
//! and hashing. No implementation code lives here.
//!
//! Types that cross into the UI (through `crates/ipc`) derive `specta::Type`.

use std::fmt;
use std::path::PathBuf;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

pub type Result<T, E = TransportError> = std::result::Result<T, E>;

/// What went wrong, as far as the caller needs to act on it. Each adapter maps its own errors
/// (rclone's messages and HTTP codes, say) to these; callers never read the message to decide.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorKind {
    /// Network or server trouble; trying again later may work. The only retryable kind.
    Transient,
    /// The path is not at the destination (or the source file is gone).
    NotFound,
    /// The destination can't do what was asked (a hash type, an operation).
    Unsupported,
    /// Credentials were refused or are missing.
    Auth,
    /// Anything else that trying again won't fix.
    #[default]
    Other,
}

/// Every transport error has a kind; only `Transient` ones are worth retrying, so `core` picks
/// backoff or failure from it.
#[derive(Debug, Clone, thiserror::Error, Serialize, Deserialize)]
#[error("{message}")]
pub struct TransportError {
    pub kind: ErrorKind,
    pub message: String,
}

impl TransportError {
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    /// A `Transient` error.
    pub fn retryable(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Transient, message)
    }

    /// An `Other` error: retrying won't help.
    pub fn fatal(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Other, message)
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::NotFound, message)
    }

    pub fn unsupported(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Unsupported, message)
    }

    pub fn is_retryable(&self) -> bool {
        self.kind == ErrorKind::Transient
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
        /// `Transport::list` works (docs/design/specs/D11).
        const LIST = 1 << 7;
        /// `Transport::move_path` works: an entry already at the destination can be renamed
        /// there ("Repair destination", docs/04).
        const MOVE = 1 << 8;
    }
}

/// Reference to a secret stored in the OS keyring. Never the secret itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub struct KeyringRef(pub String);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
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
    /// A WebDAV server (Nextcloud's own API in WebDAV mode, docs/05). `root` is relative to
    /// `url`.
    Webdav {
        /// Base URL of the user's files, e.g. `https://cloud.example.com/remote.php/dav/files/alice/`.
        url: String,
        user: String,
        /// The password (a Nextcloud app password) in the keyring.
        password: Option<KeyringRef>,
        /// Server flavour for rclone (`nextcloud`, `owncloud`, …); `None` = generic WebDAV.
        vendor: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
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
    /// Size of the filesystem or account that holds the root, when the backend reports it.
    pub total_bytes: Option<u64>,
    /// Space in use there (by anyone, not only filebridge).
    pub used_bytes: Option<u64>,
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
}

impl Default for TransferOptions {
    fn default() -> Self {
        Self { concurrency: 4 }
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

/// One file the transport is sending at the moment of a snapshot.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct InFlight {
    /// The item's `TransferItem::dest`, as text.
    pub dest: String,
    pub size: u64,
    /// Bytes of this file sent so far.
    pub bytes: u64,
    pub bytes_per_sec: f64,
    pub eta_secs: Option<u64>,
}

/// One entry of a destination folder.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteEntry {
    /// One path component, as the destination stores it.
    pub name: String,
    pub is_dir: bool,
    /// Files only.
    pub size: Option<u64>,
    /// Unix ms, when known.
    pub mtime: Option<i64>,
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
    /// Files being sent at the moment of the snapshot; empty when the transport can't tell.
    pub transferring: Vec<InFlight>,
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

/// Moves files to a destination. The contract every adapter keeps is checked by the shared
/// suite in [`contract`] (feature `contract`), run against each adapter.
#[async_trait]
pub trait Transport: Send + Sync {
    fn id(&self) -> &str;
    fn capabilities(&self) -> TransportCaps;
    /// Space, permissions, connectivity.
    async fn preflight(&self, dest: &Destination) -> Result<PreflightReport>;
    /// Sends every item to its `dest` name, creating parent directories. An error for the whole
    /// call means nothing can be said per item; otherwise each item has its own outcome, in
    /// order, and a missing source file is `NotFound`. `ItemDone` is sent as each file arrives.
    async fn transfer(
        &self,
        dest: &Destination,
        job: &TransferJob,
        progress: ProgressSink,
    ) -> Result<TransferResult>;
    /// The file's hash at the destination. `NotFound` if it isn't there; `Unsupported` if the
    /// destination can't produce this kind of hash.
    async fn hash(&self, dest: &Destination, path: &RemotePath, algo: HashAlgo) -> Result<Digest>;
    /// The entries directly inside `dir` (the destination root when `None`); only folders with
    /// `dirs_only`. `NotFound` if `dir` isn't there. Adapters that implement it advertise
    /// [`TransportCaps::LIST`]; the default answers `Unsupported`.
    async fn list(
        &self,
        _dest: &Destination,
        _dir: Option<&RemotePath>,
        _dirs_only: bool,
    ) -> Result<Vec<RemoteEntry>> {
        Err(TransportError::unsupported(
            "listing folders is not supported by this transport",
        ))
    }
    /// Renames the file or folder `from` to `to`, both already under the destination's root and
    /// normally in the same folder. Never overwrites: an entry named exactly `to` is an error.
    /// Safe on servers that store NFD and NFC (or two cases) as one name, where `from` and `to`
    /// may be "the same" entry: adapters go through an intermediate name. `NotFound` if `from`
    /// isn't there. Adapters that implement it advertise [`TransportCaps::MOVE`].
    async fn move_path(
        &self,
        _dest: &Destination,
        _from: &RemotePath,
        _to: &RemotePath,
    ) -> Result<()> {
        Err(TransportError::unsupported(
            "renaming at the destination is not supported by this transport",
        ))
    }
    /// The most this transport may use, in bytes per second across all its transfers at once,
    /// including ones already running; `None` lifts the limit. Default: no limit to apply.
    async fn set_bandwidth_limit(&self, _bytes_per_sec: Option<u64>) -> Result<()> {
        Ok(())
    }
}

#[cfg(feature = "contract")]
pub mod contract;

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
