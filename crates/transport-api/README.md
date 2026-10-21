# crates/transport-api

The `Transport` trait and its types. **Lands in:** Phase 0.

## Contents
`Transport`, `TransportCaps`, `Destination`, `RemotePath`, `PreflightReport`, `TransferJob`,
`TransferItem`, `TransferResult`, `HashAlgo`, `Digest`, `ProgressSink`, error type with a
`retryable` flag.

## Rules
- No implementation code. Stable, small, documented.
- Types that cross into the UI derive `serde` + `specta::Type`.

## Allowed dependencies
`async-trait`, `serde`, `specta`, `bitflags`, `thiserror`, `tokio` (sync primitives only).

See [docs/03-extension-contracts.md](../../docs/03-extension-contracts.md).
