# D07 — Job log and Activity

- ID: D07 · Status: **job log Built**, Activity Proposed · Phase: **1** for the job log (the Log tab; docs/06 promised it, and
  a 31-hour send that ran while the window was closed needs it), **2** for Activity (Home feed)
- Inspired by: G6 (slothUI Recent Activity), D3 (UpLink Recent), E5 (relative times)
- Layers touched: `crates/core` (what gets appended), `crates/store` (two queries),
  `crates/model` (view types), `crates/ipc` (`job_log`, `activity`), `apps/desktop` (Log tab,
  Activity section)
- Depends on: [01](../01-design-system.md) (`RelativeTime`, icons),
  [02](../02-information-architecture.md#transfer-detail-tabs) (tabs)

## Problem

- The `events` table exists and is append-only (`crates/store/src/events.rs`, with
  `events_page`), but no operation reads it. The window's log is the `live` list in
  `stores/jobs`, capped at 500 and holding only what arrived while the window was open. Close the
  window for the night and the morning log is empty (story S7).
- What gets persisted today is uneven: `state` (with every state change, in `jobs.rs`),
  `options`, `analysis`, `integration`. Retries, failures per round, verify results and the send
  window waiting are not persisted.
- There is no cross-job view of "what happened lately" (G6).

## User stories

- S7: read what happened while the window was closed.
- As the person migrating, I want a short feed of milestones across all transfers, so I know
  what happened since I last looked.

## Design

### Log tab (Transfer detail → Log)

```
Log                                                   [ All ▾ ]  [ Copy log ]
TODAY
09:14:02  ✓  Finished with no differences                       25 412 files · 393 GB
09:02:40  ⛬  Nextcloud: indexed 3 folders                       (integration)
08:51:10  ⛨  Verified 25 412 files: all identical
03:40:55  ↻  Network error: connection reset. Trying again in 2 min (attempt 3)
03:12:07  ⏾  Waiting for the send window: opens at 22:00
YESTERDAY
22:00:00  ▶  Sending started (8 at a time, verify on)
21:58:31  ✎  Analysis: 25 412 files, 312 names fixed automatically, 12 to decide
21:58:02  +  Transfer created: Fotos +2 → Nextcloud home
```

- Newest first, grouped by day. Each line has a time (`.num`), a kind icon, one sentence built
  from i18n with parameters, and an optional muted detail on the right.
- **Level filter**: All / Warnings and errors. "Copy log" copies the visible lines as plain
  text, for bug reports.
- Live: new events are added at the top as they arrive (from the `Subscribe` stream) and are
  de-duplicated against the persisted ones. Live progress events (`TransferProgress`,
  `ScanProgress` and so on) are **never** shown in the log; they belong to `JobProgress`.

### Activity (Home)

```
ACTIVITY
✓ Sent 25 412 files to Nextcloud home          2 h ago
   Photos 2019
↻ Nextcloud home unreachable, retrying          3 h ago
   Photos 2019 · attempt 3
✗ Scans stopped: destination folder missing    5 d ago
+ Added destination "Backup disk"              1 w ago
[See all in Transfers]
```

Activity is the **milestones** of every job plus profile changes, newest first. It is not every
log line: retries collapse to the first one of a run ("retrying"), and options or analysis
details are left out.

| Milestone | Comes from | Line (en) |
|---|---|---|
| Created | `created` | Created "{label}" |
| Sending started | `state` → Transferring (first time) | Started sending "{label}" to {destination} |
| Waiting for the window | `send_window_wait` (first of a run) | "{label}" waits for the send window |
| Retrying | `retry` (first of a run) | {destination} unreachable, retrying |
| Finished | `finished` | Sent {files} files to {destination} |
| Finished with warnings | `finished` | Sent {files} files to {destination}, with {n} warnings |
| Failed | `finished` | "{label}" stopped: {reason} |
| Cancelled | `finished` | Cancelled "{label}" |
| Profile added / removed | `profile` (job-less) | Added destination "{name}" / Removed destination "{name}" |

## Behaviour

### What gets appended (core)

Each row of `events` is `(job_id, at, kind, data)`. `kind` is stable (part of the persisted
format); `data` is JSON with **parameters only**, no ready-made sentences, so old logs re-render
in the user's current language. Never secrets ([07](../../07-security-reliability.md#security)):
error texts go through the same `Redacted` handling as everything else that leaves the engine.

| `kind` | When | `data` | Level | Today |
|---|---|---|---|---|
| `created` | `create_job`, `send_again` | `{ sources, profile_id, again_of? }` | info | **new** |
| `options` | `set_options`, `create_job` | the `SendOptions` | info | exists |
| `analysis` | end of Analyse | `{ files, bytes, scan_errors, skipped_symlinks, preflight, issues: {auto, pending} }` | info | exists (+ `issues`) |
| `decisions` | each `decide_all` / `decide_many`, and the carry-over of [D06](D06-transfers-list.md) | `{ rule?, decision, count, carried? }` | info | **new** |
| `state` | every state change | `{ from, to, error? }` | info / error when `to` = Failed | exists |
| `retry` | `RetryScheduled` | `{ attempt, in_secs, error }` | warn | **new** |
| `send_window_wait` | `SendWindowWait` | `{ opens_at }` | info | **new** (pairs with the send-window work) |
| `transfer_round` | end of each transfer stage run | `{ sent, failed, skipped_existing, bytes, first_error? }` | info / warn if `failed > 0` | **new** |
| `verify` | end of each verify round | `{ round, checked, ok, mismatch, missing, unverifiable }` | info / warn | **new** |
| `integration` | end of after-transfer | the `AfterTransferReport` | info / warn | exists |
| `finished` | `JobFinished` | `{ state, counts, warnings, error? }` | info / warn / error | **new** |
| `profile` | `save_profile` (new), `delete_profile` | `{ action: "added" \| "removed", profile_id, name }` | info | **new**, `job_id = NULL` |

`EntryFailed` is **not** appended per file. On a bad night that could mean 300 000 rows. The
round summary (`transfer_round.failed` and `first_error`) plus the Manifest ([D05](D05-files-and-entry-detail.md))
carry it.

### Reading

- `job_log(job_id, after_id, limit)` returns rows oldest-first after a cursor (it wraps the
  existing `events_page`). The Log tab loads backwards from the newest page: an extra
  `before_id` keyset reads pages newest first, and the window reverses them.
- `activity(before_id, limit)` reads across jobs, newest first, only the milestone kinds, and
  collapses retries: a `retry` row is a milestone only when the job's previous milestone was not
  a `retry`. That check runs in SQL with `LAG()` over `(job_id ORDER BY id)` filtered to the
  milestone kinds.
- Live merge in the window: each persisted row has an `id`, but live events don't. After a
  `JobStateChanged` or `JobFinished` arrives, the Log tab re-reads `job_log` after its last id.
  So the persisted log is the single source; live events only trigger reads.

## States

| State | Behaviour |
|---|---|
| Job with no events (old, pre-feature) | Shows what exists (`state`, `options`, `analysis`); nothing to migrate |
| Empty Activity | `EmptyState` "Nothing happened yet" ([01](../01-design-system.md#empty-states)) |
| Unknown `kind` (a newer service wrote it) | A neutral line "Event: {kind}", never an error |

## Data and contracts

### Model

```rust
// crates/model/src/views.rs
#[derive(..., specta::Type)] #[serde(rename_all = "snake_case")]
pub enum LogLevel { Info, Warn, Error }

pub struct LogEntry {
    pub id: i64,
    pub at: i64,                       // Unix ms
    pub kind: String,                  // the table's kind; the window maps it to an i18n key
    pub level: LogLevel,
    pub data: serde_json::Value,       // parameters for the sentence
}

pub struct ActivityItem {
    pub id: i64,
    pub at: i64,
    pub kind: String,
    pub level: LogLevel,
    pub job_id: Option<String>,
    pub job_label: Option<String>,     // label or default label, resolved by the service
    pub profile_name: Option<String>,
    pub data: serde_json::Value,
}
```

`level` is derived when the row is read (from `kind` and `data`), not stored, so the rules can
improve without a migration. The derivation is pure data in `crates/model` (`log_level(kind,
&data)`).

### IPC

```text
/// A job's log, oldest first after `after_id`, or newest first before `before_id`.
JobLog job_log(job_id: String, after_id: Option<i64>, before_id: Option<i64>, limit: u32) -> Vec<LogEntry>;
/// Milestones of every job and profile changes, newest first, before `before_id`.
Activity activity(before_id: Option<i64>, limit: u32) -> Vec<ActivityItem>;
```

`limit` is capped at 500.

### Store

- `events_page_before(job_id, before_id, limit)`: new, newest first.
- `activity_page(before_id, limit)`: new; the milestone filter and retry collapsing in one query.
- Index: `CREATE INDEX events_kind ON events(kind, id)` (in **M-entry-index** or its own
  migration), so the Activity query doesn't scan every progress-free row of every job.

### Core

Append at the points in the table above. **As built:** each milestone is appended right after
the change it describes (`Engine::log`), not inside the state change's transaction: the `state`
row is still written in that transaction, as before, and a crash between the two loses at most
one informative line, never state. `send_window_wait` is best effort (a failed append never
stops the wait). `finished` carries `{ state, warnings, error }` (the counts are on the job). `profile`
events are appended by `save_profile`/`delete_profile` in the engine adapter.

## i18n

Namespace `log.*`, one key per `kind` (plus variants):

| Key | en | pt |
|---|---|---|
| `log.created` | Transfer created: {sources} → {destination} | Transferência criada: {sources} → {destination} |
| `log.started` | Sending started ({concurrency} at a time, verify {verify}) | Envio iniciado ({concurrency} de cada vez, verificação {verify}) |
| `log.retry` | {error}. Trying again in {in} (attempt {attempt}) | {error}. Nova tentativa dentro de {in} (tentativa {attempt}) |
| `log.waitWindow` | Waiting for the send window: opens at {time} | À espera da janela de envio: abre às {time} |
| `log.round` | Sent {sent} files ({bytes}); {failed} failed | Enviados {sent} ficheiros ({bytes}); {failed} falharam |
| `log.verify` | Verified {checked} files: {mismatch} different, {missing} missing | Verificados {checked} ficheiros: {mismatch} diferentes, {missing} em falta |
| `log.verifyClean` | Verified {checked} files: all identical | Verificados {checked} ficheiros: todos idênticos |
| `log.finished.completed` | Finished with no differences | Concluída sem diferenças |
| `activity.sent` | Sent {files} files to {destination} | Enviados {files} ficheiros para {destination} |
| `activity.retrying` | {destination} unreachable, retrying | {destination} inacessível, a tentar de novo |
| `activity.profileAdded` | Added destination "{name}" | Destino "{name}" adicionado |

## Accessibility

- The log is a list (`<ol reversed>`) with day headings. Times are `<time datetime>`.
- New live lines are not announced one by one. One polite announcement covers a change of job state.

## Acceptance criteria

1. Start a send, close the window, let it retry twice and finish. Reopening shows `created`,
   the start, both retries, the round summaries, verify and finish in the Log tab, in order.
2. A job with 50 000 failed files appends one `transfer_round` per round, not 50 000 rows.
3. Activity lists one "retrying" line for a run of 6 consecutive retries, then the finish.
4. Switching the app language re-renders past log lines in the new language (no stored sentences).
5. No `data` field of any event kind contains a password, key or token (a test appends one of
   each kind from a profile with a keyring reference and greps the JSON).
6. Profile add and remove appear in Activity with `job_id = NULL`.

## Out of scope

- Exporting the log as a file. "Copy log" covers bug reports, and the report ([D09](D09-report-export.md))
  covers proof.
- Notifications for Activity items beyond the existing OS notification on finish.
- Retention or pruning of `events` (it's small compared with the Manifest).

## Open questions

- Should the service also record **app-level** events (service started, upgraded, rclone
  restarted) for troubleshooting? They would go in `events` with `job_id = NULL` and never show
  in Activity. The proposal is yes, behind the "Copy log" of Settings → About, later.
