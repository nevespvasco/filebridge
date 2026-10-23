# crates/host

Wiring shared by the binaries that host the engine (`apps/service`, `apps/cli`).
**Landed in:** Phase 1, Wave 3.

- **Data directory:** `default_data_dir()` (XDG on Linux, Application Support on macOS,
  `%APPDATA%` on Windows) and the single-instance lock `filebridge.lock`. One engine per data
  directory; `Access::Read` commands skip the lock.
- **rclone:** `find_rclone()` (explicit path, then next to the executable: `filebridge-rclone`,
  the installed app's sidecar, then `rclone`; then PATH, then `~/.local/bin`) and the `Rcd`
  sidecar.
- **Built-in plugins:** `RcloneTransport` (plan A), `Nextcloud` and `Immich` (ADR 0020).
  `HostOptions::transport` picks rclone, none (commands that never reach a destination), or
  the local-disk fake (`filebridge-service --fake-transport`: tests and the UI's dev mode, no
  rclone). `Host::with_registry` opens a data directory with plugins the caller brings.
- **Nextcloud wizard:** `Host::detect_nextcloud()` turns the ipc request into the plugin's
  `Nextcloud::set_up` input and saves the profile it builds (with the transport this host
  registered),
  shared by `filebridge profile add-nextcloud` and the service.
- **Immich wizard** (`src/immich.rs`, `add_immich`): keeps the API key (and an SFTP password) in
  the keyring under a fresh profile id, looks at the destination folder and its space with this
  host's transport, runs the plugin's `Immich::set_up` checks, checks with a probe file it
  writes and then removes (`Transport::remove_probe`, ADR 0029) that Immich sees the folder,
  and saves the profile only when nothing failed; otherwise the secrets go again. Tested
  against the plugin's `MockImmich`, through a whole send that ends with one library scan.
- **SSH host keys:** `scan_host_key` and `trust_host_key` (`src/host_keys.rs`, ADR 0015) answer
  with the SSH plugin's scan and append; a changed key is `host_key_changed`.
- **The engine behind the ipc seam:** `EngineOperations` implements `filebridge_ipc::Operations`
  (every operation in the catalogue) with the engine and the plugins; `Host::operations()` is
  what `filebridge-service` serves. Its tests (`tests/ipc.rs`) drive it over a real socket.

This is the only crate besides those binaries that names concrete plugins. `core` and `ipc`
stay plugin-free.
