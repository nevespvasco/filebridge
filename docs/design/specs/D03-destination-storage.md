# D03 — Destination storage

- ID: D03 · Status: **Built** (see "As built") · Phase: 2
- Inspired by: D2 (UpLink sidebar meter), E9 (CloudVault storage card), F5 (Vault & Vine stacked
  meter), G5 (slothUI donut), G7 ("Space almost expired" notice)
- Layers touched: `crates/transport-api` (two **additive** fields on `PreflightReport`),
  `crates/transport-rclone`, `crates/transport-fake`, `crates/integration-nextcloud` (optional quota),
  `crates/store` (migration **M-profile-test**), `crates/model`, `crates/core`, `crates/ipc`,
  `apps/desktop`
- Depends on: [01](../01-design-system.md) (`StorageMeter`, `Notice`)

## Problem

- `PreflightReport` carries `free_bytes` only. The rclone adapter already calls
  `operations/about` in `preflight` (`crates/transport-rclone/src/transport.rs`) and drops the
  `total` and `used` it gets back.
- The result of a connection test lives only in the window's memory. Destinations shows free space
  after an automatic test each time it opens, and Home ([D01](D01-home.md)) would have to run an
  SSH round trip per destination just to draw a tile.
- Nothing warns that a destination is filling up, during or between sends (S4).
- For Nextcloud, the disk's free space is not the limit that matters when the Nextcloud user has
  a **quota**.

## User stories

- S4: know whether the destination has room, before and during the send.
- As the person migrating, I want to see every destination's space at a glance, without
  testing each one.

## Design

### Destinations screen: profile card

```
┌───────────────────────────────────────────────────────────┐
│ ☁  Nextcloud home                       [Nextcloud · direct]│
│    demo/Backup on nc.example.lan                            │
│    ███████████████████████░░░░░░░░░  71 % used              │
│    1.7 TB used · 0.7 TB free · 2.4 TB          checked 5 min│
│    Quota for demo: 500 GB of 1 TB                           │
│                                   [Test]  [Browse]  [⋮]     │
└───────────────────────────────────────────────────────────┘
```

- `StorageMeter` in `usage` mode: one "used" segment (`--text-2`) and free in `--track`. A
  destination is not split by Category: the destination's content isn't scanned, and scanning it
  would be the Repair job's work.
- Under it: used · free · total (`.num`), and `RelativeTime` of the check. "checked 3 d ago" turns
  `--warn-strong` after 1 hour, matching [D01](D01-home.md#behaviour).
- When the Integration reports a quota (Nextcloud): a second line, "Quota for {user}: {used} of
  {total}", with its own thin meter. The **effective free space** (see below) is what every other
  screen uses.
- When only free space is known (no total): the bar is replaced by "{free} free", and the line says
  "This destination doesn't report its total size".

### Low space

| Where | When | Look |
|---|---|---|
| Profile card | effective free < 10 % of total, or < 10 GB | `Notice tone="warn"` inside the card: "Only {free} free" |
| Sidebar notice slot | any profile is low | "{name} has {free} free" → Destinations ([02](../02-information-architecture.md#sidebar)) |
| Analysis "Does it fit?" | the transfer leaves < 5 % or < 10 GB | [D02](D02-source-and-composition.md#step-3--analysis-what-youre-sending) |
| During a send | free space measured during the run drops below the bytes still to send | `IntegrationProblem`-style warning on the job: "Nextcloud home may run out of space: {free} free, {left} still to send" |

The thresholds are constants in `crates/model` (`LOW_SPACE_RATIO = 0.10`,
`LOW_SPACE_BYTES = 10 GB`) so the UI and the engine agree.

## Behaviour

### What counts as free space

`effective_free = min(transport.free_bytes, integration quota free)`, for whichever of them are
known. The same rule applies to `total` and `used`: the meter shows the tighter of the two.

### When storage is measured

Measuring means an SSH or HTTP round trip, so it never runs on a timer:

1. **Test** on a profile (exists) → stores the result.
2. **Analyse** (stage 2 already runs both preflights) → stores the result for the job's profile.
3. **After a transfer stage ends** (done, failed or paused) → one `preflight` of the transport, and
   the result is stored. This catches "the send filled the disk".
4. **During a send**: if the transport fails a file with a disk-full message, the adapter maps it
   to `ErrorKind::Other` (not retryable), and core stores a fresh preflight. The job's failure
   then says "destination is full" instead of an rclone message.
5. Explicit `profile_storage(id, refresh = true)` ("Check now").

`profile_storage(id, refresh = false)` never touches the network. It answers from the stored
result (`stale = true` when older than 1 hour).

### As built

- Built: `total_bytes`/`used_bytes` on `PreflightReport` (rclone from `operations/about`, the fake
  from fixed values, Nextcloud direct mode from `df -Pk`), the contract check, the Nextcloud quota
  from `occ user:info <user> --output=json` (`storage.quota` > 0; tighter than the disk wins), the
  kept test (migration 5, `last_test` and `last_good_test` with their times), measuring after
  Test, Analyse and every transfer stage, `profile_storage`, the Destinations meter, total and
  used in "Does it fit?", and the sidebar's low-space notice.
- Not built yet: the warning *during* a send ("may run out of space") and mapping a disk-full
  transfer error to "destination is full". Both need the transport to report the error kind;
  today a full disk fails the job with rclone's own message.

## States

| State | Card |
|---|---|
| Never tested | "Not checked yet" + Check now |
| Checking | The old numbers stay; a thin indeterminate bar on top |
| Test failed | The last good numbers greyed, with "Last check failed: {reason}" and Try again |
| Unsupported (no `about`) | "This destination doesn't report its free space" |

## Data and contracts

### Contract: `transport-api` (additive)

```rust
pub struct PreflightReport {
    pub free_bytes: Option<u64>,
    /// Size of the filesystem or account holding `root`, when the backend reports it.
    pub total_bytes: Option<u64>,
    /// Space in use there (by anyone, not just filebridge).
    pub used_bytes: Option<u64>,
    pub writable: bool,
    pub latency_ms: Option<u64>,
    pub checks: Vec<Check>,
}
```

- `PreflightReport` derives `Default`, so adapters that don't set the fields stay valid.
- Contract suite addition: if `total_bytes` and `free_bytes` are both `Some`, then
  `free_bytes <= total_bytes`; if `used_bytes` is `Some` too, then `used + free <= total` with a
  1 % tolerance (some filesystems reserve blocks).
- `transport-rclone`: read `total` and `used` from the `operations/about` answer it already gets.
  A backend that doesn't implement About (rclone returns an error) keeps today's behaviour: a
  `Warn` check, all three `None`.
- `transport-fake`: `statvfs` of its root (Unix) or fixed fixture values under test.

The integration side reuses the same `PreflightReport`: `Integration::preflight` may fill the
three fields with the user's **quota** (Nextcloud: `occ user:info <user> --output=json` in direct
mode → `quota.used`, `quota.total`, `quota.free`; a WebDAV `quota-available-bytes` PROPFIND in
Phase 2). `integration-api` needs no change.

### Model

```rust
// crates/model/src/views.rs
pub struct StorageView {
    pub profile_id: String,
    pub free_bytes: Option<u64>,      // effective
    pub total_bytes: Option<u64>,     // effective
    pub used_bytes: Option<u64>,      // effective
    /// The disk/account as the transport sees it, and the integration's quota, when different.
    pub transport: Option<SpaceView>,
    pub quota: Option<SpaceView>,
    pub checked_at: Option<i64>,      // Unix ms of the measurement
    pub stale: bool,                  // older than STORAGE_STALE_AFTER (1 h)
    pub low: bool,                    // under the LOW_SPACE_* thresholds
    /// Why the last check failed, if it did (the numbers above are then the last good ones).
    pub last_error: Option<String>,
}
pub struct SpaceView { pub free_bytes: Option<u64>, pub total_bytes: Option<u64>, pub used_bytes: Option<u64> }

// PreflightView gains the same two fields as PreflightReport.
```

### Store: migration M-profile-test

```sql
ALTER TABLE profiles ADD COLUMN last_test TEXT;      -- JSON: the ProfileTest (no secrets: it is preflight output)
ALTER TABLE profiles ADD COLUMN last_test_at INTEGER;
ALTER TABLE profiles ADD COLUMN last_good_test TEXT; -- the last test whose transport preflight passed
```

```rust
pub async fn save_profile_test(&self, id: &str, test: serde_json::Value, ok: bool) -> Result<()>;
pub async fn profile_test(&self, id: &str) -> Result<Option<StoredTest>>;
```

`Check.message` strings come from adapters. They must already be free of secrets
(`Redacted<T>`, [07](../../07-security-reliability.md#security)); the store adds no filtering of its
own, so this is a review point for every adapter.

### Core

- `Engine::test_profile` stores the result before answering.
- `run.rs`: after the transfer stage ends, `transport.preflight` → `save_profile_test`. This is
  best effort, and an error here never fails the job.
- `Engine::profile_storage(id, refresh)` builds `StorageView` (effective = the min rule above).

### IPC

```text
/// A destination's space from the last check; `refresh` runs a new check first (network).
ProfileStorage profile_storage(id: String, refresh: bool) -> StorageView;
```

A `StorageChanged { profile_id }` event is **not** added. The window re-reads `profile_storage`
after `JobFinished` and after `test_profile`, which covers every moment a measurement is taken.

## i18n

| Key | en | pt |
|---|---|---|
| `storage.used` | {used} used · {free} free · {total} | {used} usados · {free} livres · {total} |
| `storage.percent` | {p} % used | {p} % usado |
| `storage.checked` | checked {when} | verificado {when} |
| `storage.never` | Not checked yet | Ainda não verificado |
| `storage.checkNow` | Check now | Verificar agora |
| `storage.noTotal` | This destination doesn't report its total size | Este destino não indica o tamanho total |
| `storage.unsupported` | This destination doesn't report its free space | Este destino não indica o espaço livre |
| `storage.quota` | Quota for {user}: {used} of {total} | Quota de {user}: {used} de {total} |
| `storage.low` | Only {free} free | Apenas {free} livres |
| `storage.runningOut` | {name} may run out of space: {free} free, {left} still to send | {name} pode ficar sem espaço: {free} livres, faltam enviar {left} |

## Accessibility

- The meter is `aria-hidden`; the text line below it carries the numbers.
- "checked 3 d ago" in `--warn-strong` also gets an icon (`PhClockCountdown`) and a tooltip.
  Staleness is never shown by colour alone.

## Acceptance criteria

1. The rclone adapter's `preflight` against a local destination returns `total_bytes`,
   `used_bytes` and `free_bytes`, consistent within 1 % of `df` for the same path.
2. The contract suite runs the new consistency check against `transport-fake` and
   `transport-rclone`.
3. After **Test**, closing and reopening the window shows the same numbers on Destinations and
   Home without any network call (`profile_storage(id, false)`).
4. With a Nextcloud quota of 1 TB with 500 GB used on a disk with 2 TB free, every screen shows
   500 GB free (the quota) and the card shows both lines.
5. A destination at 6 GB free shows the low notice on its card and in the sidebar slot. Dismissing
   the sidebar notice hides it until the next measurement is still low.
6. After a job finishes, the profile's `last_test_at` is newer than the job's `finished_at` minus
   1 minute.

## Out of scope

- A breakdown of the destination's content by Category (would need a remote scan; that belongs
  to Repair destination, Phase 2).
- Periodic background polling of space.
- Buying or upgrading storage (rejected).

## Open questions

- Is `occ user:info` fast enough to run on every Analyse on a slow HDD host? If not, the quota is
  refreshed only on **Test** and after a send.
