# crates/host

Wiring shared by the binaries that host the engine (`apps/service`, `apps/cli`).
**Landed in:** Phase 1, Wave 3.

- **Data directory:** `default_data_dir()` (XDG on Linux, Application Support on macOS,
  `%APPDATA%` on Windows) and the single-instance lock `filebridge.lock`. One engine per data
  directory; `Access::Read` commands skip the lock.
- **rclone:** `find_rclone()` (explicit path, then next to the executable, then PATH, then
  `~/.local/bin`) and the `Rcd` sidecar.
- **Built-in plugins:** `RcloneTransport` (plan A, `Strategy::PerFile`) and `Nextcloud`.
- **Nextcloud wizard:** `detect_nextcloud()` (LXC detection over SSH, then save the profile),
  shared by `filebridge profile add-nextcloud` and the service's ipc `Extensions`.

This is the only crate besides those binaries that names concrete plugins. `core` and `ipc`
stay plugin-free.
