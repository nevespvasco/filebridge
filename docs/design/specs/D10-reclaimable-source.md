# D10 — Reclaimable source

- ID: D10 · Status: Proposed · Phase: 2
- Inspired by: F17 (Vault & Vine "Cleanup scan that surfaces reclaimable space"), adapted under
  [ADR 0011](../../adr/0011-reclaimable-source-is-shown-never-freed.md)
- Layers touched: `crates/scanner` (record mtime), `crates/core` (eligibility, re-check),
  `crates/store` (mtime in the Manifest, `reclaim_dismissed_at` from **M-job-label**),
  `crates/model`, `crates/ipc` (`reclaim_status`, `recheck_sources`, `dismiss_reclaim`),
  `apps/desktop` (panel on Overview, Home section, sidebar notice), Tauri `plugin-opener`
- Depends on: [D05](D05-files-and-entry-detail.md) (verify states, `EntryStatus`), [01](../01-design-system.md)

## Problem

[ADR 0005](../../adr/0005-fix-names-in-transit.md) says freeing the source is "manual and only offered
after a zero-difference verification". Nothing implements the *offered* part, so the user has to
judge by themselves whether a 400 GB folder can go. That is exactly the judgement filebridge exists
to make safe (story S10). Freeing the source is the whole point of a migration, and also the
one step that can lose data for good.

## User stories

- S10: know when it is safe to free space at the source.
- As the person migrating, I want to see the total I could reclaim across all my finished transfers.

## Decision this spec implements

[ADR 0011](../../adr/0011-reclaimable-source-is-shown-never-freed.md): filebridge **shows** that a
source is reclaimable and **opens it in the OS file manager**. It never deletes, moves or
empties anything at the source, and offers no button that does.

## Design

### On Transfer detail → Overview (only when the job is in a terminal state)

Eligible:

```
┌ 🧹 Space you can free at the source ───────────────────────────────────────┐
│ All 25 412 files of this transfer arrived and were verified identical at   │
│ Nextcloud home on 22 Oct, 09:14. Nothing changed at the source since       │
│ (checked 2 min ago).                                                       │
│                                                                            │
│  /home/demo/Fotos          18 210 files · 381 GB      [📂 Show in file manager] │
│  /home/demo/Vídeos            312 files ·  12 GB      [📂 Show in file manager] │
│                                                                            │
│ filebridge never deletes anything. If you want the space back, delete      │
│ these folders yourself, in your file manager.                    [Hide]    │
└────────────────────────────────────────────────────────────────────────────┘
```

Not eligible: the same card in a neutral tone, listing **why**, each reason with a way forward:

```
┌ Freeing the source isn't safe yet ─────────────────────────────────────────┐
│ ✗ Verification was off for this transfer.            [Send again with verify] │
│ ✗ 3 files are different at the destination.          [See them]            │
│ ✗ 27 files were skipped and exist only at the source. [See them]            │
│ ✗ 14 files changed at the source after the transfer. [Send again]           │
│ ! Last checked 3 days ago.                            [Check again]          │
└────────────────────────────────────────────────────────────────────────────┘
```

### Home and sidebar

- Home ([D01](D01-home.md)): a **Reclaimable** section with the sum across eligible, non-hidden
  jobs, and one row per job.
- Sidebar notice slot ([02](../02-information-architecture.md#sidebar)), lowest priority: "{size}
  verified and reclaimable".

## Behaviour

### Eligibility (computed by the service, never by the window)

A job is **reclaimable** when all of these hold:

| # | Condition | Reason code when false |
|---|---|---|
| 1 | The job is `Completed` (not `CompletedWithWarnings`, not `Failed`/`Cancelled`) | `not_completed` |
| 2 | It ran with `verify = true` | `not_verified` |
| 3 | Every file entry is `transfer_state = done` **and** `verify_state = done` | `different` (mismatch/missing), `unverifiable`, `failed` |
| 4 | No entry is `skipped` | `skipped` |
| 5 | A **source re-check** ran after `finished_at` and found the source unchanged | `not_checked`, `changed` |
| 6 | The re-check is less than 24 h old | `stale_check` |

`CompletedWithWarnings` fails condition 1 on purpose. Its warnings may be integration problems
that don't affect the bytes, but "shown as safe" must not need the user to interpret a warning.
They can resolve the warning and Send again.

Eligibility is computed **per source root** too. A job with two sources where one has a skipped
file still shows the other as reclaimable, with the reason under the blocked one.

### Source re-check

- `recheck_sources(job_id)` walks the job's source roots (scanner, read-only, no hashing) and
  compares each file's `(relative raw path, size, mtime)` with the Manifest:
  - **changed**: same path, different size or mtime;
  - **new**: at the source but not in the Manifest;
  - **gone**: in the Manifest but not at the source (doesn't block: gone files can't be lost by
    deleting the source; reported as information).
- It emits `ReclaimCheckProgress` while walking (a 300 000-file walk on an HDD takes minutes).
- The result is stored on the job (`reclaim_check` JSON: time, counts, the first 100 changed/new
  paths), so eligibility answers instantly afterwards.
- It runs on demand (**Check again**), and automatically when the panel opens if the last check
  is older than 24 h and the job finished less than 30 days ago.
- **mtime is required**: today `analyse.rs` stores `mtime: None`. This spec makes the scanner
  record mtime (seconds + nanoseconds where the OS gives them) and `analyse.rs` store it. Jobs
  analysed before that have no mtime, so their re-check falls back to size only and says so:
  "Compared by size only (transfer from an older version)". It is still eligible, but the panel
  shows that line.

### Show in file manager

Uses `revealItemInDir` from `@tauri-apps/plugin-opener` on each source root (the same capability
as [D05](D05-files-and-entry-detail.md#window), reveal only). The service performs no action;
this is a window-side read.

### Hide

**Hide** sets `reclaim_dismissed_at`, which removes the job from Home's sum and the sidebar
notice. The panel on the job stays visible (with "Hidden from Home", and Show again). Nothing
records whether the user actually deleted anything; filebridge never finds out, by design.

## States

| State | Behaviour |
|---|---|
| Checking | The card shows `ReclaimCheckProgress` ("Checking the source: 12 400 / 25 412") |
| Source root missing (disk unmounted) | Reason `source_unavailable`, "Connect the disk to check" |
| Source root missing, everything else fine | Also not eligible: a missing root can't be checked, and showing it as reclaimable invites deleting the wrong thing later |

## Data and contracts

### Model

```rust
#[derive(..., specta::Type)] #[serde(rename_all = "snake_case")]
pub enum ReclaimBlocker {
    NotCompleted, NotVerified, Different, Unverifiable, Failed, Skipped,
    NotChecked, Changed, StaleCheck, SourceUnavailable,
}
pub struct BlockerView { pub code: ReclaimBlocker, pub count: Option<u64> }

pub struct SourceReclaim {
    pub path: String,             // absolute, lossy
    pub files: u64,
    pub bytes: u64,
    pub eligible: bool,
    pub blockers: Vec<BlockerView>,
}

pub struct ReclaimStatus {
    pub job_id: String,
    pub eligible: bool,           // every source eligible
    pub bytes: u64,               // sum of eligible sources
    pub verified_at: Option<i64>, // finished_at of the verifying run
    pub checked_at: Option<i64>,  // last re-check
    pub size_only: bool,          // re-check compared sizes only (no mtimes stored)
    pub sources: Vec<SourceReclaim>,
    pub blockers: Vec<BlockerView>,  // job-level ones
    pub changed_sample: Vec<String>, // first changed/new paths, for "See them"
    pub dismissed: bool,
}

// event.rs
ReclaimCheckProgress { job_id: String, checked: u64, total: u64 },
```

### IPC

```text
/// Whether the job's sources can be freed safely, and why not; no I/O at the source.
ReclaimStatus reclaim_status(job_id: String) -> ReclaimStatus;
/// Walks the sources (read-only) and compares them with the Manifest; then answers the status.
RecheckSources recheck_sources(job_id: String) -> ReclaimStatus;
/// Hides (or shows again) the job from Home and the sidebar's reclaimable total.
DismissReclaim dismiss_reclaim(job_id: String, dismissed: bool) -> ();
```

`list_jobs`'s `JobFilter.reclaimable = true` ([D06](D06-transfers-list.md#model)) returns jobs
eligible by stored state (conditions 1–6 with the stored check), not dismissed, for Home.

### Store

- `jobs.reclaim_dismissed_at` (M-job-label) and `jobs.reclaim_check TEXT` (JSON, same migration).
- `manifest_entries.mtime` already exists; `analyse.rs` starts filling it.
- `reclaim_counts(job_id)`: per source position, the counts by `(transfer_state, verify_state)`.

### Scanner

`crates/scanner`: `Entry` gains `mtime: Option<i64>` (Unix ms) from `symlink_metadata`. No extra
system call: the metadata is already read for the size.

## i18n

| Key | en | pt |
|---|---|---|
| `reclaim.title` | Space you can free at the source | Espaço que pode libertar na origem |
| `reclaim.notYet` | Freeing the source isn't safe yet | Ainda não é seguro libertar a origem |
| `reclaim.body` | All {files} files of this transfer arrived and were verified identical at {destination} on {date}. Nothing changed at the source since ({checked}). | Os {files} ficheiros desta transferência chegaram e foram verificados idênticos em {destination} a {date}. Nada mudou na origem desde então ({checked}). |
| `reclaim.never` | filebridge never deletes anything. If you want the space back, delete these folders yourself, in your file manager. | O filebridge nunca apaga nada. Se quiser recuperar o espaço, apague estas pastas você mesmo, no gestor de ficheiros. |
| `reclaim.blocker.not_verified` | Verification was off for this transfer. | A verificação estava desligada nesta transferência. |
| `reclaim.blocker.skipped` | {n} files were skipped and exist only at the source. | {n} ficheiros foram ignorados e só existem na origem. |
| `reclaim.blocker.changed` | {n} files changed at the source after the transfer. | {n} ficheiros mudaram na origem depois da transferência. |
| `reclaim.sizeOnly` | Compared by size only (transfer from an older version) | Comparado só pelo tamanho (transferência de uma versão anterior) |

## Accessibility

- Eligible and blocked cards differ by **title text** and icon, not only by tone.
- Each blocker is a list item whose action button names the blocker ("See the 27 skipped files").

## Acceptance criteria

1. No code path in `core`, `host`, `apps/service`, `apps/cli` or `apps/desktop` removes,
   truncates or renames anything under a job's source roots (review checklist plus a test that
   runs a full job, a re-check and a reclaim status, then compares the source tree's listing and
   mtimes before and after).
2. A job verified with zero differences, re-checked within 24 h and unchanged, is eligible: Home
   sums its bytes, and the panel shows both sources with **Show in file manager**.
3. Touching one source file after the job finishes makes the next re-check report `changed` with
   that path in `changed_sample`, and **Send again** is offered.
4. A job with `verify = false` is never eligible, whatever else holds.
5. `CompletedWithWarnings` is never eligible.
6. With the source disk unmounted, the status is `source_unavailable` and nothing is shown as
   reclaimable.
7. **Hide** removes the job from Home's total and the sidebar notice, and the job's panel still
   shows it with "Show again".

## Out of scope

- Any delete, move-to-trash or "free space" button. Rejected by [ADR 0011](../../adr/0011-reclaimable-source-is-shown-never-freed.md).
- Hash-level re-checks of the source (re-hashing 400 GB to detect silent changes). Size + mtime
  is the practical signal, and verification already proved the destination.
- Tracking whether the user freed the space.
