# crates/ipc

Messages between clients (the Tauri window, later the CLI) and `filebridge-service`.
**Lands in:** Phase 1, Wave 2. See [ADR 0008](../../docs/adr/0008-service-ipc.md).

## Responsibility
- `Request` / `Response` enums mirroring the `core::Engine` API, plus an `Event` stream
  (the progress events from [docs/02](../../docs/02-architecture.md#progress-events)).
- Newline-delimited JSON framing over a local socket (`interprocess`): a Unix socket at
  `$XDG_RUNTIME_DIR/filebridge/service.sock` (0600) or a per-user named pipe on Windows.
- `Client` (connect, call, subscribe) and `serve(engine)` helpers.
- All types derive `serde` + `specta::Type`, so tauri-specta generates the TypeScript side.

## Allowed dependencies
`core` (for the types it wraps), `interprocess`, `tokio`, `serde`, `serde_json`, `specta`.
