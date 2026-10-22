# crates/transport-fake

A `Transport` adapter that copies files into a local directory (`Backend::Local`, the
destination's `root`), with knobs to fail the way real destinations do: whole calls that fail
with a retryable error, files that always fail, files written corrupted or "lost" once, a delay
per file, no hashes. **Landed in:** Phase 1.

## Who uses it
- `core`, `ipc` and the service's tests, instead of each keeping its own fake.
- `filebridge-service --fake-transport` (transport id `fake`): the UI's dev mode and tests on
  machines without rclone.

## Rules
- It keeps the same contract as rclone: `tests/contract.rs` runs
  `filebridge_transport_api::contract::check` against it.
- Test knobs are public fields; nothing here is meant for real destinations.
