# ADR 0019 — Scheduled sends

- Status: Accepted
- Date: 2026-10-23
- Details the scheduled sends that [ADR 0018](0018-scope-personal-use-scheduled-and-copy-only-sync.md)
  put in Phase 3

## Context
ADR 0018 fixed the outline: a saved send runs at set times and sends only what changed since
its last run. It runs in the service, never in the window, and waits for the daily send window
([ADR 0010](0010-bandwidth-limit-and-send-window.md)) like any send. The destination never
loses a file.

Every send today is a Job that the user starts, after seeing and approving its name changes
([02](../02-architecture.md)). A scheduled send has nobody watching when it runs. That leaves
open: what is saved, how "what changed" is known without re-sending everything, what a changed
file does to the copy already at the destination, what happens to new names that need a
decision, and what happens when a run is late, missed or still going.

The user chose (2026-10-23): keep both versions of a changed file; accept the name rules'
proposals on their own and wait only for names with no proposal; run once at start-up when
runs were missed; recurrence as weekdays and a time, or every N hours.

## Decision

### What a schedule is
- A **schedule** holds a name, its **sources** (absolute paths, like a job's), a destination
  **profile**, the send options (`target_dir`, `verify`, `concurrency`), a **recurrence** and an
  `enabled` flag.
- **Recurrence** (`model::Recurrence`), in the service machine's local time:
  - `days { days, minute }`: on the chosen weekdays at `minute` past midnight
    ("every day at 03:00", "Mon, Wed, Fri at 22:30");
  - `hours { every, minute }`: every day at `minute`, then every `every` hours until midnight
    ("every 6 hours from 00:00": 00:00, 06:00, 12:00, 18:00).
  A local time skipped by a DST change runs at the first minute after it; a repeated one runs
  once. Each schedule keeps `next_run_at` (Unix ms), computed when it is saved and after each
  run.
- Sources, profile and target folder are fixed once saved: what "changed since the last run"
  means depends on them. To change them, remove the schedule and add a new one. Name,
  recurrence, `verify` and `enabled` can change.

### Storage (store migration 12)
- `schedules` (id, name, profile, options and recurrence as JSON, enabled, `next_run_at`,
  `last_run_at`, `last_outcome`, `last_error`, timestamps) and `schedule_sources` (like
  `job_sources`).
- `schedule_files`: one row per file the schedule has sent — source root and path as raw bytes,
  size, mtime, destination path, the job that sent it. Written in the same transaction that marks
  the file `done` in a scheduled run's manifest, so it is never ahead of nor behind what was sent.
- `schedule_decisions`: the name decisions of the schedule's runs, per source path and rule,
  saved when a run starts sending.
- `jobs.schedule_id` (the run's schedule; `SET NULL` when the schedule is removed) and
  `manifest_entries.kept_as` (where the older version of a changed file was kept).
- Removing a schedule removes its three tables' rows; its runs stay in the Transfers list as
  ordinary jobs. A profile used by a schedule can't be deleted until the schedule is removed.

### A run is an ordinary Job
1. When `next_run_at` has passed, the service creates a **Transfer job** with the schedule's
   sources, profile and options, `schedule_id` set, its label the schedule's name, and
   `keep_older` set in its options (below). `last_run_at` and `next_run_at` are saved with it.
2. **Analysis** scans the sources as usual and then leaves out every file whose
   `schedule_files` row has the same size and mtime: it is **unchanged**. Folders are kept only
   on the way to a file that stays. Only new and changed files enter the manifest, so a run's
   Files tab, counts and report are what that run sent. A run with nothing new or changed is
   deleted again right away; the schedule records `nothing_changed`.
3. **Name decisions**: decisions saved in `schedule_decisions` are carried to the same paths
   and rules (and checked against the rules, like Send again). Every issue still pending that
   has a proposal is accepted. Only names with no proposal (an undecodable name, one no cut can
   fit) leave the run in **AwaitingApproval**, where the user decides them as in any send; the
   service starts the run once it is Ready. The source is never touched either way
   ([ADR 0005](0005-fix-names-in-transit.md)).
4. From there it is a normal job: Transferring, Waiting outside the send window, Retrying,
   Verifying, Integrating; every stage persisted and resumed after a restart. The bandwidth
   limit is the app-wide one. Analysis runs at the scheduled time even when the window is
   closed (it only reads local files); the transfer waits for the window.
5. A run interrupted in Analysing goes back to Draft at start-up (as any job); the service then
   analyses it again, and starts a Ready run that never started.

"Changed" is size or mtime. Hashing every file on each run would read the whole source every
time; a file changed without changing either (rare, and only by tools that restore mtimes) is
not sent. A file sent and then deleted at the destination by the user is not sent again while
it stays unchanged at the source.

### A changed file never overwrites: the older version is kept
Jobs with `SendOptions.keep_older` (every scheduled run) never replace a file at the
destination. Before each transfer batch, `core` lists the destination folders of the batch's
files. For a file whose destination name is already taken:
- by a file of the same size and the same SHA-256 (the local file hashed, the destination's
  `hash_many`): it is already there; nothing is moved;
- by any other file: it is first renamed in its folder to
  `<name> (filebridge <YYYY-MM-DD HHMM>)<ext>`, the run's start in local time (` (2)`, ` (3)`…
  when taken), through `Transport::move_path` (the checked two-hop move of
  [03](../03-extension-contracts.md), its intermediate name derived from the job and entry, so a
  move cut short is finished on resume). The new version then takes the name. The kept name is
  recorded in `manifest_entries.kept_as`, shown with the file, and given to the integration
  with the sent paths, so Nextcloud indexes both.
- A destination that can't rename (no `MOVE`), or a folder in the way, fails that file with the
  reason; it is tried again on the next run.
This covers a file changed at the source, a new file whose name is taken at the destination
(the first run into a folder that already holds files), and a renamed source file that takes an
old name. Nothing at the destination is ever deleted or overwritten with different content.
The destination grows with the older versions; tidying it is the user's, by hand.

### Overlap, missed runs, sleep
- **Overlap:** while a schedule has an unfinished run (any state but Completed, Completed with
  warnings, Failed, Cancelled), a due time doesn't start another; the schedule records
  `still_running` and moves on to its next time. Files changed meanwhile go in the next run
  after it.
- **Missed runs:** `next_run_at` is persisted. When the service starts (or the computer wakes)
  after one or more due times, the schedule runs **once**, then `next_run_at` is the next time
  after now. A paused run (by the user, or a full destination) stays paused; it counts as
  unfinished.
- The service's timer looks at the wall clock at least every minute, so suspend, DST and clock
  changes are picked up within a minute. filebridge doesn't wake the computer (as ADR 0010).
- **Disabled** schedules keep their `next_run_at` empty and never run on their own; "Run now"
  works on them.

### Where it runs
- The timer runs in `filebridge-service` only (`Engine::run_scheduler`, started next to
  `resume_unfinished`). The window and the CLI only manage schedules through the operations.
  `core` checks due schedules with `Engine::tick_schedules`, which takes the time as an argument
  (`EngineOptions.wall_clock` for the local time zone), so tests drive it with a fake clock.
- The CLI runs the engine in-process (ADR 0008), so `filebridge schedule run-now` runs one run
  there and waits for it, like `filebridge send`.
- On phones (ADR 0014) there is no service; their engine doesn't start the timer. Phase 4.

### Operations (ipc, `PROTOCOL_VERSION` 25)
- `list_schedules() -> Vec<ScheduleView>`: each schedule with its next and last run, its last
  run's job id and state, and how many files it has sent.
- `save_schedule(input: ScheduleInput) -> ScheduleView`: creates (no id) or updates one.
- `delete_schedule(id)`.
- `run_schedule_now(id) -> Option<String>`: starts a run now (the new job's id; `None` when
  nothing changed). Refused while a run is unfinished.
- Event `ScheduleRan { schedule_id, job_id, outcome }` after each due time is handled.
- `JobSummary.schedule_id` tells a scheduled run apart in the Transfers list.

### Window
- **Schedules** in the side navigation: one row per schedule (name, sources, destination,
  recurrence in words, next run, last run and its outcome with a link to its job), with
  Enable/Disable, Run now, Edit and Remove (Remove asks first and says the runs stay).
- **New schedule / Edit**: sources (picker), destination, target folder, recurrence (weekdays +
  time, or every N hours from a time), verify. It says that changed files keep the older version
  at the destination, and that runs wait for the send window.
- A scheduled run in the Transfers list and its detail shows "Scheduled" and the schedule's
  name; a file whose older version was kept shows the kept name.
- Texts in pt and en.

### Continuous sync (Phase 5) builds on this
A watched folder is a schedule whose trigger is "on change" instead of a recurrence: file
events (with a periodic scan as the fallback) set the schedule due, debounced, and the same run
is created. Everything above carries over unchanged: change detection against
`schedule_files`, carried and auto-accepted name decisions, keep-older, one run at a time (a
change during a run makes one more run after it), and ADR 0018's rules (nothing deleted at the
destination, a rename is a new file). Its ADR decides only the watching and the batching.

## Consequences
- A scheduled send costs one scan of the sources per run; only new and changed files are read
  and sent. The `schedule_files` table grows with the files a schedule has sent (one row per
  file, not per run).
- Older versions of changed files pile up at the destination under dated names. That is the
  price of never losing one.
- A run can still wait for the user when a new name has no proposal; nothing else needs them.
- `keep_older` is a send option, so it could later be offered for manual sends too.
- Rejected: replacing changed files (loses the older version), sending the new version under a
  dated name (the destination's own name keeps the stale version), comparing hashes on every run
  (reads everything), keeping full manifests per run to find changes (rows per run, not per
  file), a cron expression (hard to show and edit).
