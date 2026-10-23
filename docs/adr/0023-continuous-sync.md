# ADR 0023 — Continuous sync: a watched schedule

- Status: Accepted
- Date: 2026-10-23
- Details the copy-only continuous sync that
  [ADR 0018](0018-scope-personal-use-scheduled-and-copy-only-sync.md) put in Phase 5, on top of
  the scheduled sends of [ADR 0019](0019-scheduled-sends.md)

## Context
ADR 0018 fixed two rules: nothing is ever deleted at the destination, and a rename at the source
is a new file. ADR 0019 built scheduled sends so that continuous sync only has to decide when a
run starts: a run already sends only what is new or changed (size, mtime) since the schedule
last sent it, carries its name decisions, keeps the destination's older versions under a dated
name, and never overlaps another run of the same schedule.

What is left: how a folder is watched, how a burst of changes becomes one run instead of many,
what happens to a file still being written, what to do when the operating system can't watch a
tree (Linux's inotify has a per-user limit of watched folders), how a watch survives the
service stopping, how it plays with the send window and the bandwidth limit, and phones.

The user chose (2026-10-23): wait 1 minute without changes before a run (editable per
schedule); rescan the whole folder every hour while events work, every 10 minutes where they
don't; while the send window is closed, wait for it to open before analysing; a file still being
written waits until it stops changing.

## Decision

### A watched folder is a schedule
- `model::Recurrence` gets a third kind, **`on_change { quiet_secs }`**: the schedule runs when
  its sources change, once they have been quiet for `quiet_secs` (5 s to 1 h, 60 by default).
  Everything else in ADR 0019 carries over unchanged: sources, profile and folder fixed once
  saved; a run is an ordinary Transfer job with `schedule_id`; change detection against
  `schedule_files`; carried and auto-accepted name decisions; `keep_older`; one unfinished run
  at a time; Run now; Enable/Disable; Remove.
- No parallel system: no new job kind, no new table of runs. Switching a schedule between a
  timetable and "on change" is an ordinary edit of its recurrence.

### Watching (`core::watch`)
- `core` defines a **`FolderWatcher`** trait: watch these roots recursively and call back on a
  change. The host gives the engine an implementation (`Engine::set_watcher`); tests give it a
  fake. `core` never depends on the watching library.
- `filebridge-host` implements it with the **`notify`** crate (inotify on Linux, FSEvents on
  macOS, `ReadDirectoryChangesW` on Windows). It reports creations, writes, renames and metadata
  changes; it ignores reads and **removals** (a deletion never needs a send, ADR 0018). An event
  queue overflow (`Flag::Rescan`) counts as a change: the run is a full rescan anyway.
- The callback only notes "schedule S changed" in memory and wakes the scheduler. The scheduler
  waits a second for the rest of the burst, then folds the notes into the store with the time it
  looks, so a burst of 10 000 events is one write, not 10 000.
- Each fold saves `last_change_at`, keeps `pending_since` (the first change not in a run yet),
  and sets the schedule's `next_run_at` to `min(last change + quiet, pending_since + 15 min)`
  (or `+ quiet` when that is longer than 15 min). So a burst becomes **one run** a quiet period
  after it stops, and a folder that never stops changing still sends every 15 minutes.
- When a run is created, `pending_since` is cleared and `next_run_at` becomes the next
  **safety rescan**: 1 hour later while events work, 10 minutes later while they don't.
  A rescan is an ordinary run: it costs one scan of names, sizes and mtimes; with nothing new it
  is deleted again (`nothing_changed`), as in ADR 0019.

### Restarts and lost events
- Watching starts when the scheduler starts (the service starting) and when a watched schedule
  is saved or switched on. **Every start of a watch makes the schedule due at once**: the run is
  a full rescan, so what changed while nobody watched (the service stopped, the computer off) is
  caught up. A run left half-way is carried on as in ADR 0019; the job itself resumes at its
  persisted stage like any job.
- The service keeps no watch state that a restart would need: `last_change_at`,
  `pending_since` and `next_run_at` are in the store; the watch itself is rebuilt.

### Where events don't work: scanning instead
- Setting up a watch can fail: on Linux each folder takes one inotify watch, and a large tree
  can exceed `fs.inotify.max_user_watches`; a network share may not deliver events at all; the
  folder may be gone (an unplugged disk). A watch can also break later (a new folder past the
  limit, the root removed).
- Then that schedule **scans instead**: its safety rescan becomes every 10 minutes, and the
  window shows "Checks every 10 minutes" with the reason. A limit is retried only when the
  schedule is saved or the service restarts (setting up a watch on a huge tree walks all of it);
  any other failure is retried at each rescan.
- filebridge never raises the system limit itself (it is a system setting); the window says how
  (`sysctl fs.inotify.max_user_watches`) and that the folder works either way.
- A network share whose events don't arrive and don't fail is only caught by the hourly rescan.
  Accepted, and said in the window's help.

### A file still being written waits
- A watched run leaves out every file whose mtime is less than `quiet_secs` ago (and not in the
  future): it is still being written. They are counted (`AnalysisSummary.unsettled`), and the
  schedule is set due again `quiet_secs` later, so the file goes once it has stopped changing.
- A file that changes again while its run sends it is sent with the contents at that moment;
  its next size or mtime differs from what the run recorded, so the next run sends it again and
  keeps the version sent first under a dated name. Nothing is lost; at worst one extra version.

### Send window and bandwidth
- A watched schedule that falls due **while the send window is closed** doesn't start a run:
  it looks again every minute until the window opens (so a settings change counts at once), and
  changes keep folding into it meanwhile. One run
  then sends the day's changes in their final state (a run analysed in the morning would be stale
  by night). Run now still runs at once and waits for the window like any send.
- The bandwidth limit is the app-wide one (ADR 0010); runs share it like any send.

### One run at a time
- A watched schedule due while its last run is unfinished (sending, Waiting, Paused, awaiting
  decisions) doesn't start another and records nothing (a `still_running` outcome on every
  burst would be noise). It looks again a minute later, and at once when any job finishes, so
  changes made during a run go in one more run right after it.

### The fixed rules, as they fall out
- **A file deleted at the source** is never looked for: removals aren't events, and a run sends
  only what exists. Its copy, and every older version, stay at the destination. Its
  `schedule_files` row stays too, so the same file put back unchanged isn't sent again.
- **A file renamed at the source** has a path `schedule_files` doesn't know: it is sent as a new
  file. The copy under the old name stays. If the new name is already taken at the destination,
  keep-older renames what is there first (ADR 0019).
- The source is never touched.

### Storage (store migration 13)
- `schedules.last_change_at` and `schedules.pending_since` (Unix ms). The watch mode isn't
  stored: it is rebuilt with the watch.

### Operations (ipc, `PROTOCOL_VERSION` 34)
- No new operations. `Recurrence::OnChange`; `ScheduleView.watch` (`mode`: `events`,
  `scanning` or `idle`, the reason when scanning, `last_change_at`, `pending_since`);
  `AnalysisSummary.unsettled`; event `ScheduleChanged { schedule_id }` when a watch notes a
  change or changes mode, so the window refreshes the card.
- `idle`: this engine isn't watching it (switched off, or an engine without the scheduler: the
  CLI's in-process engine).

### CLI and window
- `filebridge schedule add --watch [--quiet SECS]`. `schedule list` says "on change", the last
  change and when the changes go; the CLI's own engine doesn't watch, so it says "watched by
  filebridge-service" (and how to raise the limit when the service reported it).
- Schedules: the recurrence picker gets "When something changes" with the quiet time. A watched
  card shows "Watching" (or "Checks every 10 minutes" and why), the last change, and "changes
  waiting" with when they go (after the quiet time, or when the send window opens).
- Texts in pt and en.

### Phones: not in Phase 5
- On phones there is no service (ADR 0014): the engine lives in the app, and Android stops
  background work that isn't a visible foreground service; a watch that must run all day would
  need a permanent notification and drains the battery. Android's `FileObserver` is also inotify
  and doesn't see changes made through other apps' storage access. iOS doesn't allow it at all.
  The phone build doesn't start the scheduler, so it neither watches nor runs schedules.
  Photos from a phone keep going by one-off sends (or, later, by a Phase 4 decision).

## Consequences
- A watched folder costs one inotify watch per folder on Linux, one wake-up per burst, one scan
  of names and sizes per run and per safety rescan (hourly, or every 10 minutes when scanning).
  Contents are read only for what is sent.
- Changes reach the destination a quiet period after they stop (1 minute by default), at most
  15 minutes after they start, or when the send window opens.
- The destination only grows: renames leave the old name, edits leave dated older versions,
  deletions leave the copy. Tidying it is the user's, by hand, as ADR 0018 says.
- `last_outcome` of a watched schedule is mostly `nothing_changed` from its rescans; the card
  shows the last change and the last run that sent something instead.
- Rejected: one watcher thread per file event becoming a run (a burst would be hundreds of
  runs), a separate "sync" job kind (duplicates the schedule machinery), `notify`'s polling
  watcher as the fallback (it keeps a second copy of the tree's metadata in memory; a run is
  already a scan), raising the inotify limit from the app (a system setting), watching on phones.
