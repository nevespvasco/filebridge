# apps/service

`filebridge-service`: the background process that owns the `Engine`, SQLite, the rclone sidecar
and the sleep inhibitor. Closing the window never stops a send. **Lands in:** Phase 1, Wave 3.
See [ADR 0008](../../docs/adr/0008-service-ipc.md).

## Responsibility
- Build the `Registry` (rclone transport, Nextcloud integration), open the store, resume
  unfinished jobs, then serve `crates/ipc` on the local socket.
- Single-instance lock (a second start exits and tells the caller the socket path).
- `filebridge-service install-user-unit` / `uninstall-user-unit` on Linux
  (`~/.config/systemd/user/filebridge-service.service`, `Restart=on-failure`).

## Allowed dependencies
`core`, `ipc`, `transport-rclone`, `integration-nextcloud`, `tokio`, `tracing-subscriber`, `anyhow`.
