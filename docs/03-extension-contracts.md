# 03 — Extension contracts

Scalability comes from three contracts (Rust traits). Everything specific to a destination or a
tool lives behind them; `core` only knows the contracts.

| Contract | Responsibility | v1 implementations | Future |
|---|---|---|---|
| `Transport` | Move bytes from source to a destination, with progress, resume and hashing | rclone (local, SFTP, WebDAV; SMB and S3 in Phase 3: [ADR 0022](adr/0022-smb-and-s3-as-rclone-backends.md)) | Google Drive, another engine |
| `NameRule` | Detect and fix name problems for a destination | NFC, forbidden characters, mojibake, length | Per-filesystem rules |
| `Integration` | Validate before and update the tool after the send | Nextcloud; Immich (Phase 3, external library: [ADR 0020](adr/0020-immich-external-library.md)) | Jellyfin, Paperless, Seafile, Syncthing |

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
| `TransportCaps` | transport-api | bitflags: `RESUME`, `HASH_SHA256`, `CHOWN`, `CHMOD`, `RENAME`, `PARTIAL_HIDDEN` |
| `Destination` | transport-api | transport id + backend config (host, port, user, root path, WebDAV URL, SMB share, S3 bucket…) + keyring refs |
| `RemotePath` | transport-api | destination-relative path, always valid UTF-8 NFC |
| `PreflightReport` | transport-api | `free_bytes: Option<u64>`, `writable: bool`, `latency_ms`, `checks: Vec<Check { id, status: Ok/Warn/Fail, message }>` |
| `TransferJob` | transport-api | job id, list of `TransferItem { source: PathBuf (raw bytes OK), dest: RemotePath, size, expected_hash }`, options (concurrency; the bandwidth limit is app-wide, `Transport::set_bandwidth_limit`) |
| `TransferResult` | transport-api | per-item outcome (`Done` / `Failed(err)`), bytes moved |
| `HashAlgo` / `Digest` | transport-api | `Sha256` only ([ADR 0017](adr/0017-sha256-for-the-manifest.md)); digest as bytes + hex |
| `ProgressSink` | transport-api | cloneable handle wrapping an mpsc sender of typed progress events; cheap to call per file |
| `Entry` | names | source path as raw bytes (`OsString` / `Vec<u8>`), decoded name if valid UTF-8, size, is_dir, parent id |
| `Issue` | names | rule id, severity (`Auto` / `NeedsDecision`), human message key, before/after bytes for display |
| `TargetProfile` | names | target OS/filesystem family, max path/name length, case-sensitive?, integration id |
| `JobContext` | integration-api | job id, profile, destination, transport caps, logger |
| `SecretResolver` | transport-api | `resolve(&KeyringRef) -> SecretValue`: read-only access to a profile's secrets, implemented by the host over the OS keyring and injected into the plugins that sign in with one (ADR 0004). Not stored is `Auth`; an unreachable keyring is `Transient` |

## Design rules

- **An integration declares its own name rules.** Nextcloud brings the NFC rule and its
  forbidden-name list (`.htaccess`, `.part`); Immich brings none (it imports any name its file
  system holds). Active rules for a job =
  general rules (that `applies_to` the target) ∪ the integration's rules.
- **An integration receives the exact list of changed paths**, so it updates only those (the
  `files:scan --path` that was done by hand). A tool that can't update by path uses the list
  only for its report: Immich's API scans a whole library.
- **`core` never lists the `Backend` variants.** What it needs of a destination's backend it
  asks the backend: `kind()` (its name in reports) and `password_mut()` (the keyring slot of
  the secret the user types, `None` without one). A new backend is a variant plus its adapter
  code, never a change in `core` ([ADR 0022](adr/0022-smb-and-s3-as-rclone-backends.md)).
- `Integration::required_caps` is checked against the chosen `Transport::capabilities` when a
  profile is created; incompatible combinations are rejected in the UI, not at send time.
- Plugins never touch SQLite or the keyring directly; `core` gives them what they need through
  `JobContext`, and the host gives the ones that sign in a `SecretResolver` (they don't depend
  on `crates/secrets`).
- Transport errors carry a **kind** (`Transient`, `NotFound`, `Unsupported`, `Auth`, `Full`,
  `Other`). `Full` (no room at the destination) pauses the job instead of failing the file
  ([ADR 0021](adr/0021-full-destination-pauses-the-job.md)).
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
  `rclone rcd` (local, SFTP, WebDAV and S3 through `rclone serve`; SMB against a real Samba when
  `FILEBRIDGE_TEST_SMB_*` is set), the Nextcloud bulk upload in front of it (`rclone serve
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

**Passed for `integration-immich`** (2026-10-23, [ADR 0020](adr/0020-immich-external-library.md)):
`git diff main -- crates/core` is empty. Besides the crate, the registry line and the UI's form,
its profile wizard added what every wizard needs around them: one ipc operation (`add_immich`),
its host side (`crates/host/src/immich.rs`), a CLI command, and a secret kind (`api-key`) in
`crates/secrets`, which `core` deletes with the profile's other secrets without knowing it.

**Passed for SMB and S3** (2026-10-23, [ADR 0022](adr/0022-smb-and-s3-as-rclone-backends.md)),
after one contract fix. `core` matched on `Backend`'s variants in two places (where a profile's
password goes, and the backend's name in a report), so any new variant was a `core` change: the
contract was wrong. `Backend::kind()` and `Backend::password_mut()` now answer for it, and `core`
calls them (one commit of its own). The SMB and S3 commit then leaves `crates/core` untouched:
two `Backend` variants, their connection strings and error kinds in `transport-rclone`, the
CLI's `add-smb` / `add-s3`, the window's form, and `smb` in phones' librclone. No new crate and
no new `Registry` line: both are rclone backends behind the same `RcloneTransport`.
