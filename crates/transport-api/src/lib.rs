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
    /// The destination has no room left: a full disk (`ENOSPC`, WebDAV `507 Insufficient
    /// Storage`) or a quota reached. Trying again won't help until someone frees space, so
    /// `core` pauses the job (resumable) instead of failing file after file.
    Full,
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

    /// A `Full` error: no space left at the destination.
    pub fn full(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Full, message)
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
        // 1 << 2 was HASH_XXH (removed by ADR 0017); left unused.
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

/// A secret's value, read for one use: wiped from memory when dropped, never printed.
pub struct SecretValue(zeroize::Zeroizing<String>);

impl SecretValue {
    pub fn new(value: String) -> Self {
        Self(zeroize::Zeroizing::new(value))
    }

    /// The value itself, for the one place that needs it (a connection, a key parser).
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for SecretValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("***")
    }
}

/// Turns a [`KeyringRef`] into its secret. Plugins never touch the keyring (docs/03): the host
/// implements this over the OS keyring and hands it to the plugins that sign in with a secret.
///
/// Errors: nothing stored is `Auth` (the user sets it again); a keyring that can't be reached
/// (locked, absent, not answering) is `Transient`, so a send waits and tries again instead of
/// failing as if the password were wrong.
#[async_trait]
pub trait SecretResolver: Send + Sync {
    async fn resolve(&self, r: &KeyringRef) -> Result<SecretValue>;
}

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
        /// The key filebridge generated for the profile, in the OS keyring (ADR 0012); used
        /// instead of `key_file` and the agent when set.
        #[serde(default)]
        key: Option<KeyringRef>,
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

/// The manifest's hash (ADR 0017). One kind today; the enum keeps room for another, which
/// needs a new ADR.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum HashAlgo {
    Sha256,
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
    /// Which of `TransferJob::source_roots` `source` is relative to.
    pub root: usize,
    /// Relative to its root; may hold non-UTF-8 bytes.
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
    /// The folders the items' sources are relative to: one call can send files from several
    /// Sources.
    pub source_roots: Vec<PathBuf>,
    pub items: Vec<TransferItem>,
    pub options: TransferOptions,
}

impl TransferJob {
    /// Where `item` is read from; `None` when its `root` isn't one of the job's.
    pub fn source_of(&self, item: &TransferItem) -> Option<PathBuf> {
        self.source_roots
            .get(item.root)
            .map(|r| r.join(&item.source))
    }
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

/// Point-in-time transfer statistics of one [`Transport::transfer`] call. The counts start
/// from zero with each call: the caller adds what earlier calls sent.
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
    /// order, and a missing source file is `NotFound`.
    ///
    /// `ItemDone { ok: true }` is sent as each file arrives, and only once it is complete under
    /// its `dest` name: the caller records it as sent on that event, so a crash afterwards never
    /// sends it again. `Stats` count from zero for this call ([`StatsSnapshot`]).
    async fn transfer(
        &self,
        dest: &Destination,
        job: &TransferJob,
        progress: ProgressSink,
    ) -> Result<TransferResult>;
    /// The file's hash at the destination. `NotFound` if it isn't there; `Unsupported` if the
    /// destination can't produce this kind of hash.
    async fn hash(&self, dest: &Destination, path: &RemotePath, algo: HashAlgo) -> Result<Digest>;
    /// [`Transport::hash`] for many files at once: one answer per path, in order, each what
    /// `hash` would say about that path. Adapters for which each `hash` costs a folder listing
    /// or a round trip override it to share them (verifying a job asks for every file it sent);
    /// the default asks `hash` for each path in turn.
    async fn hash_many(
        &self,
        dest: &Destination,
        paths: &[RemotePath],
        algo: HashAlgo,
    ) -> Vec<Result<Digest>> {
        let mut out = Vec::with_capacity(paths.len());
        for path in paths {
            out.push(self.hash(dest, path, algo).await);
        }
        out
    }
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
    /// Renames one entry at the destination, safely ([`MoveRequest`]): through the request's
    /// intermediate name (servers that store NFD and NFC, or two cases, as one name would see
    /// "unchanged" and drop the source), never onto an entry that answers to `to`'s name exactly,
    /// in another normalisation or in another case, and checked afterwards. A move cut short
    /// between its two hops is finished by the same request. `NotFound` when neither `from` nor
    /// the intermediate is there and `to` isn't the moved entry. Adapters that implement
    /// [`Transport::move_unchecked`] get this for free and advertise [`TransportCaps::MOVE`].
    async fn move_path(&self, dest: &Destination, req: &MoveRequest) -> Result<()> {
        checked_move(self, dest, req).await
    }
    /// One server-side rename of `from` (a folder when `is_dir`) to `to`, with **no** checks: it
    /// may overwrite or merge. Only [`Transport::move_path`] calls it, after its own checks.
    async fn move_unchecked(
        &self,
        _dest: &Destination,
        _from: &RemotePath,
        _to: &RemotePath,
        _is_dir: bool,
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

/// One rename at the destination ([`Transport::move_path`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MoveRequest {
    pub from: RemotePath,
    /// Normally in `from`'s folder.
    pub to: RemotePath,
    /// The intermediate name, in `from`'s folder. Deterministic (the caller derives it from its
    /// job and entry), so a move cut short between its hops is found and finished by asking
    /// again.
    pub via: RemotePath,
    /// What `from` was when the caller listed it. A move asked again after both `from` and
    /// `via` are gone is done only if `to` is this entry: same kind, and for a file the same
    /// size and modification time (when known).
    pub is_dir: bool,
    pub size: Option<u64>,
    pub mtime: Option<i64>,
}

/// Whether `e` is the entry a move request was about: same kind, and for a file the same size
/// and modification time, where both sides know them.
fn is_moved(e: &RemoteEntry, is_dir: bool, size: Option<u64>, mtime: Option<i64>) -> bool {
    fn same<T: PartialEq>(a: Option<T>, b: Option<T>) -> bool {
        a.is_none() || b.is_none() || a == b
    }
    e.is_dir == is_dir && (is_dir || (same(e.size, size) && same(e.mtime, mtime)))
}

/// [`Transport::move_path`]'s promise, over [`Transport::list`] and
/// [`Transport::move_unchecked`]: the folders are listed before the first hop, again right
/// before the second, and once more to confirm.
pub async fn checked_move<T: Transport + ?Sized>(
    t: &T,
    dest: &Destination,
    req: &MoveRequest,
) -> Result<()> {
    let MoveRequest { from, to, via, .. } = req;
    if via.parent() != from.parent() || via == from || via == to {
        return Err(TransportError::fatal(format!(
            "{via} can't be the intermediate name of {from}"
        )));
    }
    let list = |dir: Option<&str>| {
        let dir = dir.map(RemotePath::new).transpose();
        async move {
            match t.list(dest, dir?.as_ref(), false).await {
                Err(e) if e.kind == ErrorKind::NotFound => Ok(Vec::new()),
                other => other,
            }
        }
    };
    let find = |l: &[RemoteEntry], name: &str| l.iter().find(|e| e.name == name).cloned();
    // Another entry answering to `to`'s name: exactly, in another normalisation or case.
    let to_key = filebridge_names::loose_key(to.file_name());
    let clash = |l: &[RemoteEntry]| {
        l.iter()
            .find(|e| {
                e.name != from.file_name()
                    && e.name != via.file_name()
                    && filebridge_names::loose_key(&e.name) == to_key
            })
            .map(|e| e.name.clone())
    };
    let taken = |by: String| TransportError::fatal(format!("{to} is taken by {by:?}"));

    let here = list(from.parent()).await?;
    if from == to {
        return match find(&here, from.file_name()) {
            Some(_) => Ok(()),
            None => Err(TransportError::not_found(format!("{from} is not there"))),
        };
    }
    let there = if to.parent() == from.parent() {
        here.clone()
    } else {
        list(to.parent()).await?
    };
    let moving = match (find(&here, from.file_name()), find(&here, via.file_name())) {
        (Some(_), Some(_)) => {
            return Err(TransportError::fatal(format!(
                "{via} is already there, from a rename cut short; rename or remove it by hand"
            )));
        }
        (None, None) => {
            // Nothing left to move: done before, if `to` is the entry that was asked for.
            return match find(&there, to.file_name()) {
                Some(e) if is_moved(&e, req.is_dir, req.size, req.mtime) => Ok(()),
                _ => Err(TransportError::not_found(format!("{from} is not there"))),
            };
        }
        (Some(source), None) => {
            if let Some(by) = clash(&there) {
                return Err(taken(by));
            }
            t.move_unchecked(dest, from, via, source.is_dir).await?;
            source
        }
        // Cut short between the hops: finish it.
        (None, Some(mid)) => mid,
    };

    // Right before the second hop: something may have taken the name meanwhile.
    if let Some(by) = clash(&list(to.parent()).await?) {
        return Err(
            match t.move_unchecked(dest, via, from, moving.is_dir).await {
                Ok(()) => taken(by),
                Err(e) => TransportError::fatal(format!(
                    "{to} is taken by {by:?}, and {from} is left as {via}: {e}"
                )),
            },
        );
    }
    if let Err(e) = t.move_unchecked(dest, via, to, moving.is_dir).await {
        if e.is_retryable() {
            let left = format!(
                "{}; {from} is left as {via} until this is retried",
                e.message
            );
            return Err(TransportError::new(e.kind, left));
        }
        // It won't work later either: put the entry back under its old name.
        return Err(
            match t.move_unchecked(dest, via, from, moving.is_dir).await {
                Ok(()) => e,
                Err(back) => TransportError::new(
                    e.kind,
                    format!("{}; {from} is left as {via}: {back}", e.message),
                ),
            },
        );
    }

    let after = list(to.parent()).await?;
    let arrived = find(&after, to.file_name()).or_else(|| {
        after
            .iter()
            .find(|e| filebridge_names::loose_key(&e.name) == to_key)
            .cloned()
    });
    match arrived {
        Some(e) if is_moved(&e, moving.is_dir, moving.size, None) => Ok(()),
        _ => Err(TransportError::fatal(format!(
            "{to} isn't there after renaming {from}; check {via}"
        ))),
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
    fn a_secret_value_never_prints() {
        let v = SecretValue::new("hunter2".into());
        assert_eq!(format!("{v:?}"), "***");
        assert_eq!(v.expose(), "hunter2");
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
