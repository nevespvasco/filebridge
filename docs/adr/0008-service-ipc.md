# ADR 0008 — Window ↔ service IPC over a local socket

- Status: Accepted
- Date: 2026-10-21

## Context
[02 — Architecture](../02-architecture.md#process-model) says the engine runs as a process separate
from the window, so closing the window never stops a send, but it left the IPC open for Phase 1.
The UI needs request/response calls (start, pause, list jobs…) and a live stream of progress
events, with TypeScript types generated from Rust.

## Decision
- The engine (`core::Engine`) runs inside **`apps/service`** (`filebridge-service`), which owns
  SQLite, the rclone sidecar and the sleep inhibitor.
- Clients connect over a **local socket** (`interprocess` crate): a Unix socket at
  `$XDG_RUNTIME_DIR/filebridge/service.sock` with mode 0600 on Linux/macOS, and a per-user named
  pipe on Windows. Only the current user can connect.
- **Newline-delimited JSON**: `Request` / `Response` pairs with an id, plus a `Subscribe` request
  that turns the connection into an `Event` stream.
- The message types live in **`crates/ipc`** and derive `serde` + `specta::Type`; the Tauri app's
  commands forward to the IPC client, so tauri-specta generates matching TypeScript.
  *Amended (Phase 1):* the types the engine produces (`Profile`, `JobSummary`, `Event`, …) live in
  **`crates/model`**, which `ipc` re-exports; `ipc` no longer depends on `core`. Every operation
  is written once in `ipc`'s catalogue, which generates the request, the client, the dispatch and
  the Tauri commands.
- The Tauri app **starts the service if the socket is missing** (and on Linux offers to install
  it as a `systemd --user` unit).
- In Phase 1 the **CLI runs the `Engine` in-process** (headless servers have no service). It may
  become an IPC client later.

## Consequences
- The window is a thin client; a crash in the UI never affects a running job.
- One extra crate and one extra binary to ship (the service is also a Tauri sidecar).
- Only one process may open the SQLite database at a time: the CLI must refuse to run a send while
  the service is running against the same data directory (checked through the service's
  single-instance lock).
- Rejected: running core inside the Tauri process (closing the window would stop sends);
  HTTP on localhost (needs auth tokens and a port; the socket's file permissions are simpler).
