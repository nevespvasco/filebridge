# crates/transport-api

The `Transport` trait and its types. **Lands in:** Phase 0.

## Contents
`Transport`, `TransportCaps`, `Destination`, `RemotePath`, `PreflightReport`, `TransferJob`,
`TransferItem`, `TransferResult`, `HashAlgo`, `Digest`, `ProgressSink`, and `TransportError`
with an `ErrorKind` (`Transient` is the only retryable kind; `NotFound`, `Unsupported`, `Auth`,
`Other`). `KeyringRef`, and `SecretResolver`/`SecretValue`: how plugins read a profile's secret
without touching the keyring (the host implements it; ADR 0004).

## Contract suite
`contract::check(transport, dest, scratch)` (feature `contract`) is the contract as a test: names
corrected in transit with new parent directories, one outcome per item in order, a missing
source as `NotFound`, `ItemDone` per file, stats that count only the call's own files, a second
send into folders that already hold files (a changed file replaced, an unchanged one kept, new
ones next to them) with sources from two roots, hashes, `NotFound` /
`Unsupported` from `hash`. Every adapter runs it from its own tests.

## Rules
- No implementation code. Stable, small, documented.
- Types that cross into the UI derive `serde` + `specta::Type`.

## Allowed dependencies
`async-trait`, `serde`, `specta`, `bitflags`, `thiserror`, `tokio` (sync primitives only; `fs`
with `contract`), `sha2` (only with `contract`), `zeroize` (`SecretValue`).

See [docs/03-extension-contracts.md](../../docs/03-extension-contracts.md).
