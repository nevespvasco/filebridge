# crates/ipc

Messages between clients (the Tauri window) and `filebridge-service`.
**Landed in:** Phase 1 (Wave 2). See [ADR 0008](../../docs/adr/0008-service-ipc.md).

## Protocol
- Newline-delimited JSON over a local socket (`interprocess`): a Unix socket at
  `$XDG_RUNTIME_DIR/filebridge/service.sock`, inside a directory forced to 0700, or a per-user
  named pipe on Windows (`Endpoint::default_for_user`).
- Client → service: `{"id": n, "type": "<request>", …}`. Service → client:
  `{"kind": "response", "id": n, "ok": …}` or `… "err": {"code", "message"}`, plus
  `{"kind": "event", "event": …}` after a `subscribe`. Requests on one connection are handled
  concurrently.
- Error `code`s are stable: `pending_decisions`, `invalid_transition`, `not_found`, `invalid`,
  `unsupported`, `internal`, `disconnected` (client side).
- `PROTOCOL_VERSION` is 2 (`set_options` and `decide_all` were added); `ping` returns it and
  clients refuse a service that speaks another one.
- `Request` mirrors the `core::Engine` API, plus `detect_nextcloud`. `set_options` changes a
  job's `SendOptions` until it is sent (Draft, AwaitingApproval, Ready); `decide_all` applies one
  decision (not `edit`) to every pending issue of a job or of one rule and answers with the count. Plugin-specific requests
  go through the `Extensions` trait, which the service binary implements, because only it may
  know plugins.

## API
- Service: `bind(&Endpoint)` (replaces a stale socket) and `serve(listener, engine, extensions)`.
- Client: `Client::connect(&Endpoint)`, typed methods (`list_jobs`, `start`, `analyse`, …) and
  `events()`. When the connection drops, every `events()` receiver reports `Closed` and later
  calls fail at once with `disconnected`.
- The `core` and `transport-api` types that requests, responses and events carry (`Profile`,
  `SendOptions`, `JobSummary`, `Event`, `Destination`, …) are re-exported, so clients depend on
  this crate only.

## TypeScript bindings (for apps/desktop)
Every request, response and event type derives `specta::Type`. specta-typescript 0.0.13
refuses 64-bit integers (`u64` byte counts, `i64` timestamps) and `serde_json::Value` by
default. The desktop export should remap them to `number` with `specta_util::Remapper`
(byte counts and millisecond timestamps stay far below 2^53).

## Tests
`tests/roundtrip.rs` (Unix): a real socket, a real engine with a fake transport, and the
client. It drives a job through analysis, a collision decision, start, events and
completion, and checks error codes and the 0700 directory; `set_options` and `decide_all` over
the socket; and that event streams close when the service hangs up.

## Allowed dependencies
`core`, `transport-api` (for the types it re-exports), `interprocess`, `tokio`, `serde`,
`serde_json`, `specta`, `async-trait`.
