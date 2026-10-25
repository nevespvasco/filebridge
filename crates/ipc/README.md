# crates/ipc

Messages between clients (the Tauri window) and `filebridge-service`.
**Landed in:** Phase 1 (Wave 2). See [ADR 0008](../../docs/adr/0008-service-ipc.md).

## Protocol
- Newline-delimited JSON over a local socket (`interprocess`): a Unix socket at
  `$XDG_RUNTIME_DIR/filebridge/service.sock` (else `filebridge/service.sock` in this user's cache
  directory, never a shared temporary one), inside a directory of this user forced to 0700, or a
  per-user named pipe on Windows whose DACL admits only this user (`Endpoint::default_for_user`).
- `Client::connect` checks who serves the endpoint before sending anything: the peer's uid on
  Unix, the token user SID of the pipe's server process on Windows. Another user's process is
  refused with `PermissionDenied` (ADR 0008).
- Client → service: `{"id": n, "request": {"type": "<request>", …arguments}}`. The request is
  kept apart from the call's `id`, so an argument may itself be called `id`. Service → client:
  `{"kind": "response", "id": n, "ok": …}` or `… "err": {"code", "message"}`, plus
  `{"kind": "event", "event": …}` after a `subscribe`. Requests on one connection are handled
  concurrently.
- Error `code`s are stable: `pending_decisions`, `invalid_transition`, `not_found`, `invalid`,
  `unsupported`, `internal`, `keyring`, `busy` (a running transfer uses that profile: e.g.
  `install_app_key` and `remove_access` wait for it), `ssh` (an SSH server couldn't be reached),
  `host_key_changed` (`trust_host_key` refused: another key is on record for the host, or the
  server presents another key than the one confirmed), `disconnected` (client side).
- `PROTOCOL_VERSION` (its history is beside it in `src/lib.rs`, now 35); `ping` returns it and
  clients refuse a service that speaks another one.

## The operation catalogue
Every operation is written **once**, in `src/operations.rs` (`for_each_operation!`): its name,
arguments and answer type. Everything that carries an operation is generated from that list:
- the wire `Request` enum;
- the `Operations` trait, one typed method per operation. `Client` implements it over the
  socket; `filebridge_host::EngineOperations` implements it with the engine and the plugins
  (only the host may know plugins, so `detect_nextcloud` is answered there);
- the server's dispatch, which serializes each answer with its declared type;
- the desktop app's Tauri commands (`apps/desktop/src-tauri/src/commands.rs` calls
  `for_each_operation!` with its own generator).

A new operation is one line in the catalogue plus its method in `EngineOperations`; the compiler
points at every other place. Bump `PROTOCOL_VERSION` with it.

Protocol 23 added `discard_draft(job_id)` (cancel a draft the New-transfer wizard replaced and
hide it from `list_jobs` and Activity, [ADR 0030](../../docs/adr/0030-discarded-drafts.md)),
`JobSummary.discarded`, `JobPage.groups` (one `GroupCount` per `JobGroup`, ignoring the filter's
`groups`) and `probe_local_target(path) -> LocalTargetProbe`.

## API
- Service: `bind(&Endpoint)` (replaces a stale socket) and `serve(listener, handler)`, where the
  `Handler` is the `Operations` plus the engine's event stream.
- Client: `Client::connect(&Endpoint)`, every operation through `Operations` (`use
  filebridge_ipc::Operations`), and `events()`. When the connection drops, every `events()` receiver reports `Closed` and later
  calls fail at once with `disconnected`.
- The `model` and `transport-api` types that requests, responses and events carry (`Profile`,
  `SendOptions`, `JobSummary`, `Event`, `Destination`, …) are re-exported, so clients depend on
  this crate only.
- `UserUnit` (with `LoginEntry`, `UnitAgent`): not a request. What `filebridge-service
  show-user-unit --json` prints (the systemd unit, LaunchAgent or Run key value) and the
  window reads for "Start the engine when I log in"; here because both binaries depend on this
  crate and neither on the other.

## TypeScript bindings (for apps/desktop)
Every request, response and event type derives `specta::Type`. specta-typescript 0.0.13
refuses 64-bit integers (`u64` byte counts, `i64` timestamps) and `serde_json::Value` by
default. The desktop export should remap them to `number` with `specta_util::Remapper`
(byte counts and millisecond timestamps stay far below 2^53).

## Tests
`crates/host/tests/ipc.rs` (Unix), next to the engine adapter: a real socket, a real engine with
the shared fake transport, and the client. It drives a job through analysis, a collision decision, start, events and
completion, and checks error codes and the 0700 directory; `set_options` and `decide_all` over
the socket; and that event streams close when the service hangs up.

## Allowed dependencies
`model` and `transport-api` (for the types it re-exports), `interprocess`, `tokio`, `serde`,
`serde_json`, `specta`, `async-trait`. **Never `core`**: a client must not compile the engine.
