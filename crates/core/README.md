# crates/core

The heart of the app: **jobs, the 6-stage pipeline, the state machine, the `Registry` and typed
progress events.** **Landed in:** Phase 1 (Wave 2).

## Public API (`Engine`)
| Area | Methods |
|---|---|
| Profiles | `save_profile` (checks plugins and caps), `get_profile`, `list_profiles`, `delete_profile`, `test_profile` |
| Jobs | `create_job`, `job`, `list_jobs`, `list_jobs_page` (with per-group counts; never discarded drafts), `wait` |
| Stages 2–3 | `analyse` (scan + rules + preflight → `AnalysisSummary`), `rule_counts`, `issues`, `decide_issue` |
| Lifecycle | `start`, `pause`, `resume`, `cancel`, `discard_draft` (ADR 0017), `resume_unfinished` (on startup) |
| Local folders | `inspect_sources`, `probe_local_target` (the file system of a local destination folder and the name rules it needs: `statfs` + `pathconf` on macOS, `/proc/self/mountinfo` on Linux and Android, always Windows rules on Windows) |
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
