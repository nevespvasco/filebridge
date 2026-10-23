# apps/service

`filebridge-service`: the background process that owns the `Engine`, SQLite, the rclone sidecar
and the sleep inhibitor. Closing the window never stops a send. **Landed in:** Phase 1, Wave 3.
See [ADR 0008](../../docs/adr/0008-service-ipc.md).

## Commands
```
filebridge-service [run]              # foreground; default
filebridge-service status             # running? version, active jobs (exit 3 if not running)
filebridge-service install-user-unit [--no-start]   # Linux: ~/.config/systemd/user/filebridge-service.service
filebridge-service uninstall-user-unit
filebridge-service show-user-unit [--json]          # Linux: the unit as it is and as install would write it
```
Options (also as env vars): `--data-dir` (`FILEBRIDGE_DATA_DIR`), `--rclone`
(`FILEBRIDGE_RCLONE`), `--socket` (`FILEBRIDGE_SOCKET`). Logs: `FILEBRIDGE_LOG`, default
`info`.

## Behaviour
- **Startup:** take the data directory's lock (a second instance, or a CLI command that
  changes state, is refused), open the store, start rclone, register the built-in plugins
  (through `crates/host`), resume interrupted jobs, then serve `crates/ipc`.
- **Shutdown:** on SIGTERM or Ctrl-C it exits cleanly and removes the socket. Running jobs
  stay stored as running and resume on the next start. A SIGKILLed service also resumes them,
  and its rclone sidecar exits with it (Linux `PR_SET_PDEATHSIG`; not yet covered on
  macOS/Windows).
- **systemd unit:** `Restart=on-failure`, `WantedBy=default.target`. It runs this exact binary
  with any `--data-dir` / `--rclone` given at install time. Installing changes your user's
  systemd config, so it is never done automatically: from the terminal, or from the app's
  Settings → "Start the engine when I log in", which runs these same commands (`--no-start`)
  after showing `show-user-unit --json` (`filebridge_ipc::UserUnit`). Refused when run from an
  AppImage, whose files are gone once it exits.

## Tests
`tests/service.rs` runs the real binary with real rclone (skipped if rclone is missing):
- A second instance is refused.
- A send driven over the socket completes, with NFC names at the destination and events
  delivered.
- SIGTERM exits cleanly and removes the socket.
- SIGKILL mid-send, then a restart, resumes the job and finishes it byte for byte.

## Allowed dependencies
`core`, `ipc`, `host` (which wires `transport-rclone` and `integration-nextcloud`), `tokio`,
`clap`, `tracing-subscriber`, `anyhow`, `serde_json`.
