# crates/integration-api

The `Integration` trait and its types. **Lands in:** Phase 1.

## Contents
`Integration`, `JobContext`, re-exports needed from `transport-api` (`TransportCaps`,
`PreflightReport`, `RemotePath`, `ProgressSink`) and `names` (`NameRule`).

## Rules
- An integration declares its own name rules (`name_rules()`).
- `after_transfer` receives the **exact list of changed paths**.
- `required_caps()` is checked against the transport when a profile is created.

## Allowed dependencies
`transport-api`, `names`, `async-trait`, `serde`, `specta`.

See [docs/03-extension-contracts.md](../../docs/03-extension-contracts.md).
