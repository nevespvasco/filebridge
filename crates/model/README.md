# crates/model

The words the UI and the service share: `Profile`, `TargetSpec`, `JobState`, `JobKind`,
`JobSummary`, `Counts`, `SendOptions`, `AnalysisSummary`, `IssueView` (+ `IssueDecision`,
`IssueSeverity`), `DecisionInput`, `PreflightView`, `ProfileTest`, `RuleCount`, `Event`; and the
Job lifecycle's **rules** as pure data (`rules.rs`: `Op::allowed_in`, `can_go`,
`job_operations`). **Landed in:** Phase 1.

## Who uses it
- `core` produces these types and enforces the rules (`lifecycle.rs`: locks, persisting); it
  re-exports everything, so `filebridge_core::Profile` still works.
- `ipc` carries them and re-exports them; the desktop app gets them through `ipc` and never
  compiles `core`, `store` or SQLite.
- The UI reads the operation table as `JOB_OPERATIONS` (generated TS constant).

## Rules
- Pure data and pure functions: no I/O, no store, no engine.
- Every type derives `serde` + `specta::Type`: renaming a field or variant is a protocol change
  (bump `filebridge_ipc::PROTOCOL_VERSION`).

## Allowed dependencies
`transport-api` (`Destination`, preflight types), `names` (`TargetProfile`), `serde`,
`serde_json`, `specta`, `thiserror`.
