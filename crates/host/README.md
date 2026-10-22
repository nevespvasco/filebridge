# crates/host

Wiring shared by the binaries that host the engine (`apps/service`, `apps/cli`).
**Landed in:** Phase 1, Wave 3.

- **Data directory:** `default_data_dir()` (XDG on Linux, Application Support on macOS,
  `%APPDATA%` on Windows) and the single-instance lock `filebridge.lock`. One engine per data
  directory; `Access::Read` commands skip the lock.
- **rclone:** `find_rclone()` (explicit path, then next to the executable, then PATH, then
  `~/.local/bin`) and the `Rcd` sidecar.
- **Built-in plugins:** `RcloneTransport` (plan A, `Strategy::PerFile`) and `Nextcloud`.
  `HostOptions::transport` picks rclone, none (commands that never reach a destination), or
  the local-disk fake (`filebridge-service --fake-transport`: tests and the UI's dev mode, no
  rclone). `Host::with_registry` opens a data directory with plugins the caller brings.
- **Nextcloud wizard:** `Host::detect_nextcloud()` turns the ipc request into the plugin's
  `Nextcloud::set_up` input and saves the profile it builds (with the transport this host
  registered),
  shared by `filebridge profile add-nextcloud` and the service.
- **The engine behind the ipc seam:** `EngineOperations` implements `filebridge_ipc::Operations`
  (every operation in the catalogue) with the engine and the plugins; `Host::operations()` is
  what `filebridge-service` serves. Its tests (`tests/ipc.rs`) drive it over a real socket.

This is the only crate besides those binaries that names concrete plugins. `core` and `ipc`
stay plugin-free.
