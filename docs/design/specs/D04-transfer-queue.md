# D04 — Transfer queue (files in flight)

- ID: D04 · Status: **Built** · Phase: **1** (it answers "is it stuck?" during the first real
  multi-hour sends, which the Phase 1 gate exercises)
- Inspired by: B3, B4 (upload modal rows with % and a check), F16 (Vault & Vine Transfers queue)
- Layers touched: `crates/transport-api` (an **additive** field on `StatsSnapshot`),
  `crates/transport-rclone`, `crates/transport-fake`, `crates/model` (event fields and a new
  event), `crates/core` (`run.rs` forwarder), `apps/desktop`
- Depends on: [01](../01-design-system.md) (`CategoryBar`, `.num`)

## Problem

`TransferProgress` carries totals only (bytes and files done/total, speed, ETA). The "current
file" listed for it in [02 — Architecture](../../02-architecture.md#progress-events) was never
built. In a 31-hour send with 8 files in flight, nothing on screen tells a slow disk (one huge
video creeping along) from a stuck send. Today the only file names the window ever sees come
from `EntryFailed` (story S2).

rclone already knows: `core/stats` for the job's stats group returns a `transferring` array
(name, size, bytes, speed, eta per file). `transport-rclone/src/stats.rs` reads only the totals.

## User stories

- S2: see which files are being sent right now, and how fast each one goes.
- As the person migrating, I want to see the last files that arrived, so I can tell the send is
  moving even when the overall % barely changes.

## Design

On Transfer detail → Overview (and in wizard step 5), under `JobProgress`, while the job is
`Transferring`:

```
SENDING NOW (8)                                              48 MB/s total
▌ IMG_2041.jpg              Fotos/2019/Verão         4.1 MB   ███████░░ 71 %   6.2 MB/s
▌ VID_0007.mov              Fotos/2019/Verão         2.3 GB   ██░░░░░░░ 18 %  21.0 MB/s · 2 min
▌ contrato final.pdf        Documentos               0.4 MB   █████████ 99 %   1.1 MB/s
…
JUST FINISHED                                                        ▸ show
✓ IMG_2040.jpg  ✓ IMG_2039.jpg  ✓ Notas.txt  ✓ IMG_2038.jpg  … (+1 204 in the last minute)
```

- One row per file in flight. At most `SendOptions.concurrency` rows (8 by default), so there is
  no virtualisation. The order is stable (by start time), so rows don't jump around.
- Row: `CategoryBar` (left edge), the **destination** file name (bold) and its folder (muted
  `.path`), the size, a `ProgressBar`, %, speed, and ETA when it is over 1 minute. Every number
  is `.num`.
- A row whose name was changed by a Name rule shows `PhTextAa` with the tooltip "Renamed from
  {source name}", which comes from the Manifest.
- Clicking a row opens Entry detail ([D05](D05-files-and-entry-detail.md)).
- **Just finished**: a collapsed strip with the last 20 file names that arrived (check icon, no
  per-file numbers) and a count for the last minute. It is a ring buffer kept in the window.
  Nothing new is persisted: the Manifest already records each Done entry.
- In other states the section explains instead of listing:

| State | Section shows |
|---|---|
| `Retrying` | "Paused by a network error. Next try in {countdown}" (existing retry countdown); the rows of the last snapshot, greyed |
| `Waiting` | "Waiting for the send window: opens at {time}" (from `SendWindowWait`) |
| `Paused` | "Paused. Files that were half-sent will start again from where filebridge can resume them." |
| `Verifying` | Hidden (`VerifyProgress` already has its own bar) |

### Compact form (Home)

[D01](D01-home.md) shows the first two names and "+N more" under each running job, with no bars.

## Behaviour

- **Rate**: the transport already polls `core/stats` every 500 ms (`spawn_poller`). The in-flight
  list rides on the same `TransferProgress` event, so no extra request or event stream is added.
- **Window size**: `in_flight` holds at most `concurrency` entries, each about 150 bytes in JSON.
  At 2 events per second that is about 2.4 KB/s, well inside what the socket handles.
- **Mapping names back**: rclone reports the destination remote path. Core keeps a `HashMap<dest,
  idx>` for the current batch (it already has `idx_of` in `run.rs`), so each in-flight row carries
  its Manifest `idx`.
- **Just finished**: core already gets `ProgressEvent::ItemDone` and saves those entries in
  batches every 500 ms. On the same tick it emits one `EntriesDone` with up to 20 of them, plus a
  count of the rest. The window keeps the last 20.
- rclone names each file in flight by its **source** path. The adapter maps that path to the
  destination name, so the rows show the final names.

## States

| State | Behaviour |
|---|---|
| Starting (no snapshot yet) | "Starting…" with an indeterminate bar |
| Snapshot with an empty `transferring` list mid-run (between batches) | Keep the last rows for up to 2 s, then "Preparing the next files…" |
| Service disconnected | Rows freeze, greyed; the app-wide banner explains |

## Data and contracts

### Contract: `transport-api` (additive)

```rust
/// One file the transport is sending right now.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct InFlight {
    /// Destination-relative path as the transport names it (`RemotePath` text).
    pub dest: String,
    pub size: u64,
    pub bytes: u64,
    pub bytes_per_sec: f64,
    pub eta_secs: Option<u64>,
}

pub struct StatsSnapshot {
    // … existing fields …
    /// Files in flight at the moment of the snapshot; empty if the transport can't tell.
    pub transferring: Vec<InFlight>,
}
```

- Additive: `StatsSnapshot` derives `Default`, so adapters that don't fill it stay valid.
- `transport-rclone/src/stats.rs`: read `transferring[]` → `name` → `dest`, `size`, `bytes`,
  `speed`, `eta` (null → `None`). **As built:** for `operations/copyfile` rclone names the
  **source** path, not the destination one (checked with rclone v1.75.2), so the adapter maps
  both an item's source path and its destination name to the destination name
  (`InFlightNames`), and leaves out names that aren't items of the job.
- `transport-fake`: report the files of its current parallel batch with simulated bytes, so the
  UI and the core forwarder are testable without rclone.
- Contract suite addition: while a multi-file transfer runs, at least one snapshot has a
  non-empty `transferring`, and each `dest` is one of the job's item destinations. This check is
  skipped for adapters that declare they can't report it (an empty list throughout is allowed).

### Model

```rust
// crates/model/src/event.rs
TransferProgress {
    // … existing fields …
    /// Files in flight now (at most the job's concurrency), oldest first.
    in_flight: Vec<InFlightView>,
},
/// Files that arrived since the last event (at most 20 listed; `more` counts the rest).
EntriesDone { job_id: String, entries: Vec<DoneView>, more: u64 },

// crates/model/src/views.rs
pub struct InFlightView {
    pub idx: Option<u64>,          // Manifest entry, when core could map it
    pub name: String,              // destination file name
    pub dir: String,               // destination folder (relative to the profile root)
    pub category: FileCategory,    // from D02; `Other` before D02 lands
    pub renamed_from: Option<String>,
    pub size: u64,
    pub bytes: u64,
    pub bytes_per_sec: f64,
    pub eta_secs: Option<u64>,
}
pub struct DoneView { pub idx: u64, pub name: String, pub category: FileCategory }
```

`EntriesDone` is advisory, like every progress event ([`event.rs`](../../../crates/model/src/event.rs)
doc comment): a slow subscriber may miss some, and that is fine. It is **not** written to the
`events` table.

### Core

`run.rs`, in the forwarder that already handles `ProgressEvent::Stats` and `ItemDone`:

- Build `in_flight` from `s.transferring` with the batch's `dest → idx` map. `renamed_from` comes
  from the Manifest entry's raw name when it differs from the destination name, lossily decoded.
- Collect `ItemDone` idx values into a `Vec` and, on each 500 ms tick, emit `EntriesDone` (first
  20, `more` = the rest).

No store, IPC operation or migration changes.

## i18n

| Key | en | pt |
|---|---|---|
| `queue.title` | Sending now ({n}) | A enviar agora ({n}) |
| `queue.total` | {speed} total | {speed} no total |
| `queue.finished` | Just finished | Acabados de chegar |
| `queue.lastMinute` | +{n} in the last minute | +{n} no último minuto |
| `queue.renamed` | Renamed from {name} | Renomeado de {name} |
| `queue.starting` | Starting… | A começar… |
| `queue.between` | Preparing the next files… | A preparar os próximos ficheiros… |
| `queue.planB` | Names are fixed at the end of this transfer | Os nomes são corrigidos no fim desta transferência |
| `queue.paused` | Paused. Files that were half-sent will start again from where filebridge can resume them. | Em pausa. Os ficheiros a meio continuam de onde o filebridge os puder retomar. |

## Accessibility

- The list is a `<table>` with a caption. Progress cells have `role="progressbar"` with
  `aria-valuenow`.
- The queue is **not** a live region (it would be read twice a second). The existing throttled
  `JobProgress` announcement covers progress.

## Acceptance criteria

1. With `transport-fake` sending 50 files at concurrency 4, every `TransferProgress` carries ≤ 4
   `in_flight` entries, and each has a non-null `idx` that matches a Manifest entry.
2. With the rclone adapter sending a 2 GB file and small ones over local SFTP, the 2 GB file
   stays as one row and its % rises over successive events.
3. A file renamed by the NFC rule shows the rename icon, and its tooltip has the original name.
4. "Just finished" lists the last 20 arrivals and counts the rest. It survives switching tabs, but
   not a window restart (it is not persisted).
5. No change to the `events` table or the operation catalogue (checked in review).

## Out of scope

- **Cancel or skip one file** (B4's ✕). Skipping mid-send changes the Manifest, needs its own
  operation (`skip_entry(job_id, idx)`, allowed in `Transferring` and `Paused`), and must be
  reflected in verify and the report. Later; see the open question.
- A full history of every arrival (that is the Manifest: [D05](D05-files-and-entry-detail.md)).
- Per-file speed graphs.

## Open questions

- Is **skip this file** worth building? It helps when one huge file blocks a slot on a slow link.
  But the file then stays Skipped, and only a new transfer or Send again
  ([D06](D06-transfers-list.md#send-again)) brings it back.
