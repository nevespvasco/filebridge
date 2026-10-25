# crates/store

SQLite persistence: **jobs, manifests, profiles, issues, events, settings, schedules.** **Landed in:** Phase 1 (1b).

## Design
- One dedicated thread owns the `rusqlite` connection. `Store` is a cheap `Clone` handle; every
  method is `async` and sends a closure to that thread, so writes are serialised and the tokio
  runtime never blocks. `Store::call` is public for queries `core` needs that aren't wrapped yet.
- File databases use WAL and `synchronous=NORMAL`; foreign keys are on.
- Migrations are embedded SQL tracked by `PRAGMA user_version` (`src/schema.rs`). Append new ones;
  never edit a shipped migration. A database newer than the app is refused.
- Rows are plain data. `core` owns the domain enums (job state, kind) and stores them as text;
  the store only defines what its own columns need (`EntryState`, `Decision`).
- Raw source paths are `BLOB`s, so non-UTF-8 names survive. `u64` values use rusqlite's
  `fallible_uint` (anything above `i64::MAX` is an error, not a silent wrap).

## API
| Area | Methods |
|---|---|
| Profiles | `upsert_profile`, `get_profile`, `list_profiles`, `delete_profile` (fails while a job uses it) |
| Jobs | `insert_job` (with source roots), `get_job`, `list_jobs`, `list_jobs_filtered` and `job_state_counts` (the Transfers list; never discarded drafts, ADR 0030), `list_unfinished_jobs` (resume on startup), `change_job_state` (`StateChange.discarded` sets `discarded_at`), `set_job_retry`, `set_job_options` |
| Manifest | `insert_entries` (batched, one transaction), `set_dest_paths`, `set_transfer_states`, `reset_transfer_states` (e.g. partial/failed → pending on resume, mismatch → pending to resend), `set_verify_states`, `entries_page`, `files_with_transfer_state`, `manifest_counts` |
| Issues | `insert_issues`, `clear_issues`, `issues_page` (by rule), `issue_summary`, `pending_issue_count`, `decide_issue` |
| Events | `append_event`, `events_page`, `last_event` (newest of a kind: integration checkpoints) |
| Settings | `get_setting` / `set_setting`, `get_setting_json` / `set_setting_json` |
| Pinned sources | `pinned_sources` (in pin order), `set_pinned` (pin or unpin; answers how many are pinned) |
| Schedules (ADR 0019) | `insert_schedule`, `update_schedule`, `get_schedule`, `list_schedules`, `due_schedules`, `next_schedule_time`, `record_schedule_run`, `delete_schedule`; `sent_files` / `sent_file_count` (written by `set_transfer_states` when a scheduled run's file is `done` or `skipped`); `save_schedule_decisions` / `carry_schedule_decisions`; `latest_run`, `unfinished_runs`, `delete_job`; `set_kept_as` / `kept_versions` |

Lists use keyset pagination (`after_idx` / `after_id` + `limit`), so the UI never loads a whole
manifest.

## Rules
- Never stores secrets: profile `config` holds keyring references only (ADR 0004).
- `core` commits every job stage transition before the next stage begins.

## Allowed dependencies
`rusqlite` (bundled), `serde`, `serde_json`, `thiserror`, `tokio` (sync). No workspace crates.

See [docs/02-architecture.md](../../docs/02-architecture.md#persistence-model-sqlite-store-crate).
