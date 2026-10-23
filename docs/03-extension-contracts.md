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
    // Items name their source by root (`TransferJob::source_roots`), so one call sends files
    // of several Sources. `ItemDone` only once a file is complete under its name: core records
    // it as sent then. `Stats` count from zero for each call.
    async fn transfer(&self, job: &TransferJob, progress: ProgressSink) -> Result<TransferResult>;
    async fn hash(&self, path: &RemotePath, algo: HashAlgo) -> Result<Digest>;
    // One answer per path, in order, each what `hash` would say; verify asks in batches. The
    // default calls `hash` per path; adapters whose `hash` costs a folder listing share it.
    async fn hash_many(&self, paths: &[RemotePath], algo: HashAlgo) -> Vec<Result<Digest>>;
    async fn list(&self, dir: Option<&RemotePath>, dirs_only: bool) -> Result<Vec<RemoteEntry>>; // LIST
    // Safe rename at the destination (MOVE, "Repair destination"): the shared `checked_move`
    // over `list` and the adapter's one-hop `move_unchecked` (see below).
    async fn move_path(&self, req: &MoveRequest) -> Result<()>;
    async fn move_unchecked(&self, from: &RemotePath, to: &RemotePath, is_dir: bool) -> Result<()>;
    // App-wide, applied to running transfers too; default no-op (ADR 0010)
    async fn set_bandwidth_limit(&self, bytes_per_sec: Option<u64>) -> Result<()>;
}

pub trait NameRule: Send + Sync {
    fn id(&self) -> &str;
    fn applies_to(&self, target: &TargetProfile) -> bool;
    fn check(&self, entry: &Entry) -> Option<Issue>;     // what is wrong; `after` is the fix
}

#[async_trait]
pub trait Integration: Send + Sync {
    fn id(&self) -> &str;
    fn required_caps(&self) -> TransportCaps;
    fn name_rules(&self, settings: &Value) -> Vec<Box<dyn NameRule>>; // rules the tool imposes, per profile
    fn check_profile(&self, dest: &Destination, settings: &Value) -> Result<()>; // on every save; default: accept
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
| `TransferJob` | transport-api | job id, list of `TransferItem { source: PathBuf (raw bytes OK), dest: RemotePath, size, expected_hash }`, options (concurrency; the bandwidth limit is app-wide, `Transport::set_bandwidth_limit`) |
| `TransferResult` | transport-api | per-item outcome (`Done` / `Failed(err)`), bytes moved |
| `HashAlgo` / `Digest` | transport-api | `Sha256` / `Xxh3_128`; digest as bytes + hex |
| `ProgressSink` | transport-api | cloneable handle wrapping an mpsc sender of typed progress events; cheap to call per file |
| `Entry` | names | source path as raw bytes (`OsString` / `Vec<u8>`), decoded name if valid UTF-8, size, is_dir, parent id |
| `Issue` | names | rule id, severity (`Auto` / `NeedsDecision`), human message key, before/after bytes for display |
| `TargetProfile` | names | target OS/filesystem family, max path/name length, case-sensitive?, integration id |
| `JobContext` | integration-api | job id, profile, destination, transport caps, logger |
| `SecretResolver` | transport-api | `resolve(&KeyringRef) -> SecretValue`: read-only access to a profile's secrets, implemented by the host over the OS keyring and injected into the plugins that sign in with one (ADR 0004). Not stored is `Auth`; an unreachable keyring is `Transient` |

## Design rules

- **An integration declares its own name rules.** Nextcloud brings the NFC rule and its
  forbidden-name list (`.htaccess`, `.part`); Immich will bring its own. Active rules for a job =
  general rules (that `applies_to` the target) ∪ the integration's rules.
- **An integration receives the exact list of changed paths**, so it updates only those (the
  `files:scan --path` that was done by hand).
- `Integration::required_caps` is checked against the chosen `Transport::capabilities` when a
  profile is created; incompatible combinations are rejected in the UI, not at send time.
- Plugins never touch SQLite or the keyring directly; `core` gives them what they need through
  `JobContext`, and the host gives the ones that sign in a `SecretResolver` (they don't depend
  on `crates/secrets`).
- Transport errors carry a **kind** (`Transient`, `NotFound`, `Unsupported`, `Auth`, `Full`,
  `Other`). `Full` (no room at the destination) pauses the job instead of failing the file
  ([ADR 0017](adr/0017-full-destination-pauses-the-job.md)).
  Only `Transient` is retried; `core` decides on the kind and never reads the message. Each
  adapter maps its own errors to kinds (rclone: `transport-rclone/src/client.rs`, `kind_of`).
  Integration errors still carry a `retryable` flag.
- **Renames at the destination are one algorithm, not one per adapter.** `move_path` takes a
  `MoveRequest { from, to, via, is_dir, size, mtime }`; its default (`checked_move` in
  `transport-api`) lists the folder, refuses a `to` that another entry answers to (exactly, in
  another normalisation or in another case: `filebridge_names::loose_key`), moves to the
  caller's deterministic intermediate `via`, lists again right before the second hop, moves to
  `to`, and lists once more to confirm. A move cut short between the hops is finished by the same
  request; one already done counts only if `to` is the same entry (kind, size, mtime). A second
  hop that fails for good puts the entry back under `from`. Adapters only implement the
  unchecked one-hop `move_unchecked` (rclone: `operations/movefile` or `sync/move`), which may
  overwrite, so nothing else calls it.
- **Every `Transport` adapter runs the shared contract suite** (`filebridge_transport_api::
  contract::check`, and `check_move` for `MOVE`, feature `contract`): rclone against a real
  `rclone rcd` (local, SFTP and WebDAV), the Nextcloud bulk upload in front of it (`rclone serve
  webdav`, and a real Nextcloud when `FILEBRIDGE_TEST_NEXTCLOUD_*` is set), and the local-disk
  fake (`crates/transport-fake`) that `core`, `ipc` and the service tests use. A new adapter
  adds the same one-function test.

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
