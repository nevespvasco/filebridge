# ADR 0003 — SQLite for state

- Status: Accepted
- Date: 2026-10-21

## Context
Jobs last many hours (31 h in the original migration) and must survive app restarts, crashes and
reboots, resuming at any of the 6 pipeline stages. Manifests can hold hundreds of thousands of rows.

## Decision
Store jobs, stages, manifests, issues/decisions, profiles, event log and settings in **SQLite**
(`crates/store`), with embedded migrations and a single writer task.

## Consequences
- Every stage transition is durable before the next stage starts.
- Large manifests are queryable and paginated for the UI.
- Rejected: **JSON files** (no atomic partial updates, poor at large manifests).
- Secrets are **not** stored here — only keyring references (see ADR 0004).
