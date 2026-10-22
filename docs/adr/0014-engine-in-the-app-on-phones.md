# ADR 0014 — On phones the engine runs in the app, with rclone linked in

- Status: Accepted
- Date: 2026-10-22
- Amends: [ADR 0008](0008-service-ipc.md) (engine in a separate service) and
  [ADR 0002](0002-rclone-behind-transport.md) (rclone as a sidecar), for phone builds only

## Context
On desktop the engine runs in `filebridge-service`, a detached process the window talks to over a
local socket, and rclone runs as a child process (`rclone rcd`) driven over HTTP. Neither works on
a phone:

- Android and iOS do not let an app start detached processes or ship its own executables the way
  the desktop does, and Tauri does not support sidecars on mobile.
- What keeps work alive is different: an Android **foreground service** (a notification the user
  sees), and on iOS only a short grace period after the app leaves the screen.

The code already has the seams: the Tauri commands call `ipc`'s `Operations` trait, which
`host::EngineOperations` implements in-process (the CLI uses it), and `transport-rclone` reaches
rclone through a single `call(method, json) -> json`, the same shape as librclone's `RcloneRPC`.

## Decision
- **Phone builds run the engine inside the app process**: the Tauri commands use
  `host::EngineOperations` under `cfg(mobile)`. The desktop keeps the service and the socket.
- **Android keeps the process alive with a foreground service** that shows the send's progress and
  holds a partial wake lock and a Wi-Fi lock while a Job is busy (Analysing, Transferring,
  Retrying, Waiting, Verifying, Integrating). Its type is `specialUse`: `dataSync` is capped at
  six hours a day on Android 15, and a migration can take longer.
  **iOS** sends while the app is open; a Job left mid-send resumes when the app returns.
- **rclone is linked in as librclone** (Go `c-shared` for Android, `c-archive` for iOS) and called
  through its C API, loaded at run time (`libloading`). `transport-rclone` has an `RcCall` trait
  with two implementations, `Rcd` (desktop, HTTP) and `Librclone` (phones, feature `librclone`);
  the rc methods and their JSON stay the same, and both pass the transport contract suite.
  `sidecars/librclone` builds rclone's own C shims with only the backends a Destination can name
  (local, SFTP, WebDAV), which keeps the library at ~19 MB instead of ~107 MB.
- **Secrets on Android** go to the app's Keystore vault (an AES key that never leaves the Android
  Keystore; only ciphertext in the app's private storage), passed to `Host::open_with_secrets`.
  iOS uses the Keychain through the keyring crate.

## Consequences
- `core` does not change; the seams live in `transport-rclone`, `host` and the Tauri app.
- The layering rule holds at the contract level (UI → Tauri commands → `ipc::Operations`); the
  phone build wires `crates/host` into the app, which `CLAUDE.md` and docs/08 will say.
- A second toolchain (Go with the NDK, later Xcode) builds librclone; pinned in `sidecars/`.
- If Android stops the foreground service, the Job resumes like after a crash: every Stage is
  persisted (the Phase 1 `kill -9` test).
- Rejected: an IPC service process on the phone (not allowed, no gain); calling librclone through
  gomobile's Java/Objective-C bindings (adds a JNI hop; Rust can call the C API directly); a
  native Rust SFTP/WebDAV transport now (loses rclone's resume, hashing and backends; kept as plan B
  if librclone fails, and it would be a new plugin crate without changing `core`).
