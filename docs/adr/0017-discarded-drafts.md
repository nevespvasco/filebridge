# ADR 0017 — A draft the wizard replaces is discarded, not shown as cancelled

- Status: Accepted
- Date: 2026-10-24

## Context
The New-transfer wizard creates a job (Draft) to analyse the chosen sources for the chosen
destination. When the user goes back and changes the sources or the destination after
"Analyse", the wizard can't reuse that job: its manifest, issues and decisions belong to the old
choice. It called `cancel` on the old job and created a new one.

The old job then showed in the Transfers list under "Cancelled", counted in the totals, and its
"finished" milestone appeared in Activity, although the user cancelled nothing: they only changed
their mind before sending. A few back-and-forths left as many phantom cancelled transfers.

The records can't simply be deleted: the rule is that a job's history is kept, and every stage is
persisted (docs/02).

## Decision
- A new operation, **`discard_draft(job_id)`**, allowed only where nothing has been sent: Draft,
  AwaitingApproval and Ready (`Op::DiscardDraft` in `model::rules`, exported in
  `JOB_OPERATIONS`).
- It moves the job to **Cancelled** exactly as `cancel` does (the same transition, the same
  `state` and `finished` events, `finished_at` set) and, in the same store transaction, sets
  **`jobs.discarded_at`** (migration 8). The `finished` event carries `discarded: true`.
- A discarded job is **kept, never deleted**. `job(id)` returns it, with
  `JobSummary.discarded = true`; its log is intact.
- `list_jobs` leaves discarded jobs out of every group, out of `total` and out of the per-group
  counts; Home's queries are `list_jobs` too. Activity leaves out every milestone of a discarded
  job. The CLI's `jobs` goes through the same `list_jobs`.
- `cancel` doesn't change: a user who cancels a job, sent or not, still sees it under
  "Cancelled".

## Consequences
- The wizard calls `discard_draft` instead of `cancel` when it replaces its job; the Transfers
  list shows only what the user did.
- Discarded rows accumulate in SQLite. They are small (no manifest beyond the analysis), and a
  later clean-up can remove them without changing any contract.
- A discarded job can't come back: it is Cancelled, a terminal state. The wizard creates a new
  draft instead, as it already did.
