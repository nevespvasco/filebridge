# crates/store

SQLite persistence: **jobs, manifests, profiles.** **Lands in:** Phase 1.

## Responsibility
- Schema + embedded migrations, run at startup.
- Tables: `profiles`, `jobs`, `job_sources`, `manifest_entries`, `issues`, `events`, `settings`.
- Single writer task; batched inserts for manifests (hundreds of thousands of rows).
- Paginated queries for UI lists (issues by rule, job log, manifest).

## Rules
- Never stores secrets — only keyring references from `secrets`.
- Every job stage transition is committed before the next stage begins.

## Allowed dependencies
`rusqlite` (bundled) or `sqlx`, `serde`. Domain types are mirrored as plain rows; no dependency on
`core`.

See [docs/02-architecture.md](../../docs/02-architecture.md#persistence-model-sqlite-store-crate).
