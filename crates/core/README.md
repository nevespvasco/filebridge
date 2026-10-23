# crates/core

The heart of the app: **jobs, the 6-stage pipeline, the state machine, the `Registry` and typed
progress events.** **Landed in:** Phase 1 (Wave 2).

## Public API (`Engine`)
| Area | Methods |
|---|---|
| Profiles | `save_profile` (checks plugins and caps), `get_profile`, `list_profiles`, `delete_profile`, `test_profile` |
| Jobs | `create_job`, `job`, `list_jobs`, `wait` |
| Stages 2–3 | `analyse` (scan + rules + preflight → `AnalysisSummary`), `rule_counts`, `issues`, `decide_issue` |
| Lifecycle | `start`, `pause`, `resume`, `cancel`, `resume_unfinished` (on startup) |
| Schedules (ADR 0019) | `save_schedule`, `list_schedules`, `get_schedule`, `delete_schedule`, `run_schedule_now`, `tick_schedules(now)` (testable: the time is an argument), `run_scheduler` (the service's timer) |
| Events | `subscribe` → `broadcast::Receiver<Event>` |

All types that cross into the UI derive `specta::Type`.

## How a job runs
- **Analysis.** Each source root is itself an entry, so `/home/u/Acme` lands as `Acme/…`.
  Names go through the general rules and the integration's rules. Two more issue kinds come
  from core itself: `collision` (two entries would get the same destination path; proposes
  ` (2)`) and `encoding` (an undecodable name with no fix). Both need a decision.
- **Start.** Decisions are applied to the manifest: final paths are recomputed, skipped
  subtrees are marked, and duplicate final paths are refused.
- **Transfer.** Files go in batches (`batch.rs`): one `Transport::transfer` per page of
  `batch_size` Pending files, from however many Sources (`TransferJob::source_roots`). Each file
  the transport reports finished is saved on the next 0.5 s tick, so a crash or a pause resends
  at most that tick's files. Retryable failures back
  off 30 s → 1 min → … → 15 min (±10 %) forever; a fatal per-file error marks that file failed
  and the job finishes with warnings.
- **Verify** (when `SendOptions::verify`). SHA-256 of the source is compared with
  `Transport::hash`. Mismatched or missing files are resent, for at most `max_verify_rounds`
  rounds. A destination that can't hash at all gives a warning, not resends.
- **Integrate.** `Integration::after_transfer` receives exactly the files sent. Problems turn
  into warnings; a failed tool update doesn't hide that the files arrived.
- **Scheduled runs** (`schedules.rs`, `keep.rs`, ADR 0019) are Transfer jobs with
  `schedule_id`: the analysis leaves out what the schedule sent before and hasn't changed,
  carries its name decisions and accepts proposals; with `SendOptions::keep_older` the transfer
  renames a destination file that other contents would overwrite to a dated name first.
- Every state change is validated (`JobState::can_go_to`), persisted, logged as an event and
  broadcast. A sleep inhibitor is held while a runner works (`inhibit.rs`: logind on Linux,
  `caffeinate` on macOS, `SetThreadExecutionState` on Windows; the Windows code is untested).

## Allowed dependencies
`transport-api`, `integration-api`, `names`, `scanner`, `store`. **Never** a concrete
`transport-*` / `integration-*` crate.

## Tests
`cargo test -p filebridge-core`: unit tests, plus `tests/engine.rs`, which runs end to end
with a local-copy fake transport. It covers NFD names, collisions, retries, fatal file errors,
verify-and-resend, an unhashable destination, pause/resume mid-batch, resume after a crash,
integration problems and cancel.

See [docs/02-architecture.md](../../docs/02-architecture.md) and [docs/03-extension-contracts.md](../../docs/03-extension-contracts.md).
