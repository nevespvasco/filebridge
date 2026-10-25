# D06 — Transfers list (search, filters, labels, Send again)

- ID: D06 · Status: **Built** · Phase: 2
- Inspired by: A7, A8 (count badge, search, filter), C4 (Message → label), D6, D7 (⋮ row menu;
  "Copy to" → send again elsewhere), D9 (filters), D13 ("know when … changed" → send only what
  changed), D15 (shortcuts for things you forget → labels), E5 (recent with relative times)
- Layers touched: `crates/model` (job groups in `rules.rs`, `Op::SetLabel`, view types),
  `crates/store` (migration **M-job-label**, filtered query, decisions copy), `crates/core`
  (`send_again`), `crates/ipc` (`list_jobs` changed, `set_label`, `send_again`), `apps/desktop`
  (`JobsView`, `StepReview`, `RowMenu`)
- Depends on: [01](../01-design-system.md) (`DataTable`, `FilterChips`, `SearchField`, `RowMenu`,
  `RelativeTime`)

## Problem

- `JobsView.vue` is a table paged by offset, 50 at a time, newest first. It has no search, no
  filter and no sort. After a few months of sends, finding "the Photos one from March" means
  paging (story S6).
- A Job has no name: the list shows the first source's base name plus "+N". Two sends of
  "Fotos" to different places look the same.
- Sending the same folder again (to bring a copy up to date, or to a second destination) means
  redoing the wizard by hand: pick the folders, pick the profile, decide every name issue again.
  If an issue is decided **differently** the second time, the destination ends up with two
  copies of one file under two names (S12).
- Actions on a job are only reachable from Job detail.

## User stories

- S6: find an old transfer by name, destination or state.
- S12: send the same folder again later, sending only what changed.
- As the person migrating, I want to give a transfer a name I'll recognise ("Photos 2019 →
  Nextcloud").
- As the person migrating, I want to send the same folders to a second destination without
  picking them again.

## Design

### List

```
Transfers                                                         [ New transfer ]
Every send, with what it is doing now. Click one to see its files and log.

[🔍 Search by name, folder or destination…]   Destination: [All ▾]   Sort: [Newest ▾]
(All 214) (Active 2) (Needs you 1) (Not sent 3) (Finished 205) (Cancelled 3)

 Transfer                      Destination       State            Progress            Size     When      
 Photos 2019 → Nextcloud       Nextcloud home    ● Sending        ███████░░ 61 %      393 GB   started 3 h  ⋮
   Fotos +2 folders                                               48 MB/s · 52 min
 Work                           Backup disk       ✓ Done           25 412 files        3.1 GB   2 d ago      ⋮
 Scans                          Nextcloud home    ✗ Failed         —                   18 GB    5 d ago      ⋮
```

- **Transfer** column: the label in bold. Without one, the default label (below). The second
  line, muted, gives the sources as today (`Fotos +2 folders`).
- **Progress**: the bar and % while running, with speed and ETA as a second line (they move out
  of their own columns). When finished: the file count; when failed: "—".
- **When**: `RelativeTime` of `finished_at`, or "started …" of `created_at` while running.
- **Size**: total bytes of the analysis (`.num`, right-aligned). Columns that can't fit go
  first: Size hides when the table is under 860px wide, and under 700px each row becomes a card that
  still shows the size and when ([02](../02-information-architecture.md#window-sizes)).
- `RowMenu` (⋮):

| Item | Shown when | Does |
|---|---|---|
| Open | always | `/jobs/:id` |
| Continue in wizard | `Draft`, `AwaitingApproval`, `Ready` | as today on Job detail |
| Pause / Resume | per `JOB_OPERATIONS` (exists) | `pause` / `resume` |
| Rename… | always | inline edit of the label (`set_label`) |
| **Send again** | terminal states | `send_again(job_id, None)` → wizard |
| **Send again to another destination…** | terminal states | profile picker → `send_again(job_id, Some(profile))` → wizard |
| Export report… | terminal states | [D09](D09-report-export.md) |
| Copy transfer id | always | clipboard (for the CLI's `job <id>`) |
| Cancel… | per `JOB_OPERATIONS` | `ConfirmDialog` (exists), `danger` style, last in the menu |

There is no "Delete transfer": a transfer's Manifest is the proof of the migration, and
removing it is out of scope (see open questions).

### Job groups

Chips filter by **group**, not by raw state. The groups are pure data in
`crates/model/src/rules.rs`, next to the other lifecycle rules, so Home ([D01](D01-home.md)) and
the CLI use the same sets:

| Group | States |
|---|---|
| `active` | Analysing, Transferring, Retrying, Waiting, Verifying, Integrating, Paused |
| `needs_you` | AwaitingApproval, Failed |
| `not_sent` | Draft, Ready |
| `finished` | Completed, CompletedWithWarnings |
| `cancelled` | Cancelled |

Every state belongs to exactly one group (unit test over `JobState::ALL`).

### Label

- Review step (`StepReview.vue`) gets one optional field above the summary: **Name this
  transfer** with a placeholder showing the default.
- Default label (when none is set; computed in the window, not stored): `{first source name}` if
  there is one source, `{first source name} +{n}` otherwise. The destination is not in it: it has
  its own column.
- 1–120 characters, trimmed, with no line breaks. An empty value clears it.
- Shown in: the Transfers list, Job detail's title, Home, Activity ([D07](D07-job-log-and-activity.md)),
  the OS notification on completion, the tray tooltip, and the report ([D09](D09-report-export.md)).
- Rename at any time, including after the job has finished (`Op::SetLabel` is allowed in every
  state and changes no state).

### Send again

```
Send again: "Photos 2019"
Same folders, same destination, same options. Only files that are new or changed since
the last time will travel; files already there and identical are skipped.
Name decisions you made last time are reused.               [Cancel]  [Continue]
```

1. `send_again(job_id, profile_id)` creates a new **Draft** job: the same sources (copied from
   `job_sources`), the same options, the same profile unless another one is given, and the label
   copied with nothing appended (the date tells them apart).
2. The window opens the wizard on that draft, and the wizard runs **Analyse** right away (step 3).
3. **Decisions carry over**: after the analysis, for each new Issue whose `(source position,
   raw_path, rule)` matches an Issue of the original job that has a non-pending Decision, that
   Decision (and custom name) is copied. Only new Issues stay pending. Without this, a name edited
   to "X" last time would get the default fix this time, and the destination would hold two
   copies of the file under two names.
4. Carried-over decisions are marked in the Analysis step ("Decided last time", a muted chip),
   and they can be changed.
5. What travels: the transport skips files that are already at the destination and identical
   (rclone's `copyfile` compares size and modification time, or hash when the backend has it).
   Changed and new files are sent; the Manifest of the new job still lists **every** file, so
   verification covers the whole folder again.
6. **Nothing is deleted at the destination**, even for files removed from the source since the
   last send. This is the existing rule (no sync, no move), and the dialog doesn't promise
   otherwise.
7. To another destination: the Name rules of the new profile apply (another naming family may
   find other issues). Decisions are carried only for rules that exist in both.

## Behaviour

- Filters, sort and destination are in the route query (`?group=active&profile=…&sort=size`) so
  Back restores them. The search text is kept in memory only.
- Paging: `DataTable` with offset paging (the number of jobs is small, thousands at most), 50
  per page, loaded as the user scrolls. The total count is shown on the "All" chip.
- Live: rows of running jobs update from `TransferProgress`/`JobStateChanged` (exists in
  `stores/jobs`). A job that changes group (say, finishes) stays in place until the next
  re-query, so rows don't jump while the user is reading. A "1 transfer changed: Refresh" link
  appears instead.
- Search: case-insensitive substring over the label, every source path, and the profile name.

## States

| State | Behaviour |
|---|---|
| No transfers at all | `EmptyState` (exists, restyled; [01](../01-design-system.md#empty-states)) |
| No match | "No transfer matches" + Clear filters |
| Loading / error / disconnected | [01 — States](../01-design-system.md#states) |

## Data and contracts

### Model

```rust
// crates/model/src/rules.rs
#[derive(..., specta::Type)] #[serde(rename_all = "snake_case")]
pub enum JobGroup { Active, NeedsYou, NotSent, Finished, Cancelled }
impl JobGroup { pub fn states(self) -> &'static [JobState]; pub fn of(state: JobState) -> Self; }

// Op gains SetLabel (allowed in every state) and SendAgain (allowed in terminal states);
// job_operations() exports "set_label" and "send_again".

// crates/model/src/views.rs
pub struct JobFilter {
    pub query: Option<String>,
    pub groups: Vec<JobGroup>,          // empty = all
    pub profile_id: Option<String>,
    pub reclaimable: bool,              // D10
    pub sort: JobSort,                  // Newest | Oldest | Largest
}
pub struct JobPage {
    pub items: Vec<JobSummary>,
    pub total: u64,
    /// One per JobGroup::ALL, in order: the count with this filter's query, profile and
    /// reclaimable, whatever its groups (each chip shows its count while another is selected).
    pub groups: Vec<GroupCount>,       // protocol 35
}
pub struct GroupCount { pub group: JobGroup, pub total: u64 }

pub struct JobSummary {
    // … existing fields …
    pub label: Option<String>,
    /// Set by send_again: the job this one repeats.
    pub again_of: Option<String>,
    /// A draft the New-transfer wizard replaced (ADR 0030); never listed.
    pub discarded: bool,
}

pub struct IssueView {
    // … existing fields …
    /// The decision was copied from the job this one repeats.
    pub carried_over: bool,
}
```

### IPC

```text
/// A page of jobs matching `filter`, with the total that matches.
ListJobs list_jobs(filter: JobFilter, limit: u32, offset: u32) -> JobPage;      // changed
/// Names a job (any state); `None` or blank clears it.
SetLabel set_label(job_id: String, label: Option<String>) -> ();
/// A new draft with the job's sources and options, to `profile_id` or the same profile.
SendAgain send_again(job_id: String, profile_id: Option<String>) -> String;
```

`list_jobs` changes shape, so `PROTOCOL_VERSION` is bumped. The CLI's `jobs [--limit]` builds a
default filter.

Discarded drafts ([ADR 0030](../../adr/0030-discarded-drafts.md)) are in no group, in no count
and in no total; `job(id)` still returns them. The chip counts come from one
`SELECT state, COUNT(*) … GROUP BY state` with the same conditions minus the states
(`Store::job_state_counts`).

### Store: migration M-job-label

```sql
ALTER TABLE jobs ADD COLUMN label TEXT;
ALTER TABLE jobs ADD COLUMN again_of TEXT REFERENCES jobs(id) ON DELETE SET NULL;
ALTER TABLE jobs ADD COLUMN reclaim_dismissed_at INTEGER;   -- D10
ALTER TABLE issues ADD COLUMN carried_over INTEGER NOT NULL DEFAULT 0;
CREATE INDEX jobs_state ON jobs(state, created_at);
CREATE INDEX jobs_profile ON jobs(profile_id, created_at);
```

- `list_jobs_filtered(filter, limit, offset) -> (Vec<JobRow>, u64)`: `WHERE state IN (…) AND
  (?profile IS NULL OR profile_id = ?profile) AND (?q IS NULL OR label LIKE ?q OR EXISTS (SELECT 1
  FROM job_sources s WHERE s.job_id = jobs.id AND CAST(s.path AS TEXT) LIKE ?q) OR profile_id IN
  (SELECT id FROM profiles WHERE name LIKE ?q))`, with sort by `created_at` or by total bytes
  (`(SELECT SUM(size) FROM manifest_entries WHERE job_id = jobs.id AND is_dir = 0)`; acceptable
  for thousands of jobs; cache the sum in a column if it shows up in profiles).
- `set_job_label(id, label)`.
- `copy_decisions(from_job, to_job, rules: &[&str]) -> u64`: an `UPDATE issues … FROM` that
  matches by `(source position, raw_path, rule)` through `manifest_entries`, sets the decision and
  custom name, and sets `carried_over = 1`, in one transaction.

### As built

- `copy_decisions` copies every decided issue of the earlier job onto the new job's issue with
  the same entry (source position + raw path) and rule. No rule list is passed: an issue only
  exists in the new job when its rule applies to the new profile. Carried decisions the new
  profile's Name plan refuses go back to pending (`reset_issues`).
- The list pages with Previous / Next and a count ("1–50 of 214"), not on scroll.
- `Export report…` joins the row menu with D09. `reclaimable` in `JobFilter` answers nothing
  until D10's source re-check exists.
- The row menu (`JobActions`) is shared by the list and Transfer detail. It renders in a layer
  over the page, so the table's scroll box doesn't clip it.

### Core

- `Engine::send_again(job_id, profile_id)`: checks `Op::SendAgain`, creates the draft (sources,
  options, label, `again_of`) and records an `options` event plus an `again` event in the new
  job's log ([D07](D07-job-log-and-activity.md)).
- `Engine::analyse`: when the job has `again_of`, after inserting the issues, call
  `copy_decisions` with the rules active for this job's profile, then compute
  `pending_decisions` as usual (so a fully carried-over analysis lands directly in `Ready`).

## i18n

| Key | en | pt |
|---|---|---|
| `jobs.search` | Search by name, folder or destination… | Procurar por nome, pasta ou destino… |
| `jobs.group.active` | Active | Ativas |
| `jobs.group.needs_you` | Needs you | Precisa de si |
| `jobs.group.not_sent` | Not sent | Por enviar |
| `jobs.group.finished` | Finished | Concluídas |
| `jobs.group.cancelled` | Cancelled | Canceladas |
| `jobs.menu.again` | Send again | Enviar de novo |
| `jobs.menu.againTo` | Send again to another destination… | Enviar de novo para outro destino… |
| `review.label` | Name this transfer (optional) | Dar um nome a esta transferência (opcional) |
| `again.explain` | Only files that are new or changed since the last time will travel; files already there and identical are skipped. | Só seguem os ficheiros novos ou alterados desde a última vez; os que já lá estão iguais são ignorados. |
| `again.decisions` | Name decisions you made last time are reused. | As decisões sobre nomes da última vez são reaproveitadas. |
| `issues.carried` | Decided last time | Decidido da última vez |

## Accessibility

- Chips are a `role="group"` labelled "Show"; each is a toggle button with its count in its name.
- The row menu's trigger is labelled "Actions for {label}".
- The inline label edit is a real `<input>` with Enter to save and Esc to cancel; the result is
  announced ("Renamed").

## Acceptance criteria

1. Every `JobState` belongs to exactly one `JobGroup` (unit test over `JobState::ALL`).
2. With 1 000 fixture jobs, searching "Fotos" returns only jobs with "Fotos" in a label, a source
   path or a profile name, and the "All" chip shows the matching total.
3. A label set in Review appears in the list, Job detail, Home, the notification and the report.
   Renaming a completed job works and leaves its state unchanged.
4. **Send again** on a completed job of 1 000 files where 3 changed and 2 were added: the new job's
   Manifest has 1 002 files, the transport moves only 5 (bytes moved ≈ their size), and
   verification covers all 1 002.
5. A name **edited** to "Ata (final).pdf" in the first job is pre-decided the same way in the
   resend (`carried_over = true`), and the destination never gets a second copy under the
   default name.
6. **Send again to another destination** with a Windows-family profile carries decisions only for
   rules active in both profiles, and leaves the new Windows-only issues pending.

## Out of scope

- Scheduled or repeated "Send again": that is the open decision on scheduled sends and
  continuous sync ([10](../../10-risks-and-open-decisions.md#open-decisions)). Send again is the
  manual step towards it.
- Deleting or archiving transfers from the list.
- Tags. A label covers the need (D15) without a tag model.

## Open questions

- Should the history ever be pruned? A job of 300 000 entries is about 100 MB of SQLite. A
  "Remove from list (keeps the report file)" action could come after [D09](D09-report-export.md)
  makes the proof portable.
