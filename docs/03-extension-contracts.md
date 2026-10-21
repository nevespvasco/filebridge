# 03 — Extension contracts

Scalability comes from three contracts (Rust traits). Everything specific to a destination or a
tool lives behind them; `core` only knows the contracts.

| Contract | Responsibility | v1 implementations | Future |
|---|---|---|---|
| `Transport` | Move bytes from source to a destination, with progress, resume and hashing | rclone (local, SFTP, WebDAV) | SMB, S3, Google Drive, another engine |
| `NameRule` | Detect and fix name problems for a destination | NFC, forbidden characters, mojibake, length | Per-filesystem rules |
| `Integration` | Validate before and update the tool after the send | Nextcloud | Immich, Jellyfin, Paperless, Seafile, Syncthing |

## Traits

```rust
#[async_trait]
pub trait Transport: Send + Sync {
    fn id(&self) -> &str;
    fn capabilities(&self) -> TransportCaps; // resume, hashes, chown, rename
    async fn preflight(&self, dest: &Destination) -> Result<PreflightReport>; // space, permissions, connectivity
    async fn transfer(&self, job: &TransferJob, progress: ProgressSink) -> Result<TransferResult>;
    async fn hash(&self, path: &RemotePath, algo: HashAlgo) -> Result<Digest>;
}

pub trait NameRule: Send + Sync {
    fn id(&self) -> &str;
    fn applies_to(&self, target: &TargetProfile) -> bool;
    fn check(&self, entry: &Entry) -> Option<Issue>;     // what is wrong
    fn propose(&self, entry: &Entry) -> Option<String>;  // suggested name
}

#[async_trait]
pub trait Integration: Send + Sync {
    fn id(&self) -> &str;
    fn required_caps(&self) -> TransportCaps;
    fn name_rules(&self) -> Vec<Box<dyn NameRule>>; // rules the tool imposes
    async fn preflight(&self, ctx: &JobContext) -> Result<PreflightReport>;
    async fn after_transfer(
        &self,
        ctx: &JobContext,
        changed: &[RemotePath],
        progress: ProgressSink,
    ) -> Result<()>;
}
```

## Supporting types to define

These live in `transport-api`, `integration-api` and `names` (shared primitives may go into a small
`types` module of `transport-api` so `integration-api` can re-use them). All public types derive
`serde` + `specta::Type` when they cross into the UI.

| Type | Crate | Shape (first draft) |
|---|---|---|
| `TransportCaps` | transport-api | bitflags: `RESUME`, `HASH_SHA256`, `HASH_XXH`, `CHOWN`, `CHMOD`, `RENAME`, `PARTIAL_HIDDEN` |
| `Destination` | transport-api | transport id + backend config (host, port, user, root path, WebDAV URL…) + keyring refs |
| `RemotePath` | transport-api | destination-relative path, always valid UTF-8 NFC |
| `PreflightReport` | transport-api | `free_bytes: Option<u64>`, `writable: bool`, `latency_ms`, `checks: Vec<Check { id, status: Ok/Warn/Fail, message }>` |
| `TransferJob` | transport-api | job id, list of `TransferItem { source: PathBuf (raw bytes OK), dest: RemotePath, size, expected_hash }`, options (bandwidth, checksum, owner/mode) |
| `TransferResult` | transport-api | per-item outcome (`Done` / `Failed(err, retryable)`), bytes moved |
| `HashAlgo` / `Digest` | transport-api | `Sha256` / `Xxh3_128`; digest as bytes + hex |
| `ProgressSink` | transport-api | cloneable handle wrapping an mpsc sender of typed progress events; cheap to call per file |
| `Entry` | names | source path as raw bytes (`OsString` / `Vec<u8>`), decoded name if valid UTF-8, size, is_dir, parent id |
| `Issue` | names | rule id, severity (`Auto` / `NeedsDecision`), human message key, before/after bytes for display |
| `TargetProfile` | names | target OS/filesystem family, max path/name length, case-sensitive?, integration id |
| `JobContext` | integration-api | job id, profile, destination, transport caps, logger, keyring access (scoped) |

## Design rules

- **An integration declares its own name rules.** Nextcloud brings the NFC rule and its
  forbidden-name list (`.htaccess`, `.part`); Immich will bring its own. Active rules for a job =
  general rules (that `applies_to` the target) ∪ the integration's rules.
- **An integration receives the exact list of changed paths**, so it updates only those (the
  `files:scan --path` that was done by hand).
- `Integration::required_caps` is checked against the chosen `Transport::capabilities` when a
  profile is created; incompatible combinations are rejected in the UI, not at send time.
- Plugins never touch SQLite or the keyring directly; `core` gives them what they need through
  `JobContext`.
- Errors carry a `retryable` flag so `core` decides between backoff and failure.

## Registry

```rust
pub struct Registry {
    transports: HashMap<String, Arc<dyn Transport>>,
    integrations: HashMap<String, Arc<dyn Integration>>,
    name_rules: Vec<Arc<dyn NameRule>>, // general rules
}
```

- Built once at startup by the app binary:
  `registry.add_transport(RcloneTransport::new(..)); registry.add_integration(Nextcloud::new(..));`
- **In v1, plugins are compiled into the app** and registered here. A new plugin = a new crate +
  one line in the `Registry` setup.
- **Dynamic plugins (WASM with Extism) come only after the contracts stabilise** — before that
  they only add cost. See [ADR 0006](adr/0006-compiled-in-plugins.md).

## Phase 3 acceptance test

"A new plugin enters without changing `core`": adding e.g. `transport-smb` or
`integration-immich` must only touch the new crate, the app's registry setup and the UI's
plugin-settings form. A diff touching `crates/core` fails the test.
