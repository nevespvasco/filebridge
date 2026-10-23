# apps/service

`filebridge-service`: the background process that owns the `Engine`, SQLite, the rclone sidecar
and the sleep inhibitor. Closing the window never stops a send. **Landed in:** Phase 1, Wave 3.
See [ADR 0008](../../docs/adr/0008-service-ipc.md).

## Commands
```
filebridge-service [run]              # foreground; default
filebridge-service status             # running? version, active jobs (exit 3 if not running)
filebridge-service install-user-unit [--no-start] [--launcher APP]   # start at login (below)
filebridge-service uninstall-user-unit
filebridge-service show-user-unit [--json] [--launcher APP]          # the entry as it is and as install would write it
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
- **Start at login** ([ADR 0028](../../docs/adr/0028-login-start-on-every-desktop-os.md)): the
  `*-user-unit` commands manage the per-user entry each OS starts programs from at login
  (`src/login.rs`). It runs this exact binary with any `--data-dir` / `--rclone` given at
  install time. Installing changes your user's login setup, so it is never done automatically:
  from the terminal, or from the app's Settings → "Start the engine when I log in", which runs
  these same commands (`--no-start`) after showing `show-user-unit --json`
  (`filebridge_ipc::UserUnit`, whose `kind` says which entry). Refused when run from an
  AppImage, whose files are gone once it exits, or from an app macOS translocated.
  - **Linux** (`src/unit.rs`): `~/.config/systemd/user/filebridge-service.service`,
    `Restart=on-failure`, `WantedBy=default.target`; `uninstall` also stops it.
  - **macOS** (`src/launch_agent.rs`): `~/Library/LaunchAgents/app.filebridge.service.plist`,
    `RunAtLoad`, started again only after a failed exit (5 s apart), output in
    `~/Library/Logs/filebridge-service.log`. Without `--no-start` it is loaded now
    (`launchctl bootstrap gui/<uid>`); `uninstall` boots it out, which stops the engine.
  - **Windows** (`src/run_key.rs`): the value `filebridge-service` under
    `HKEY_CURRENT_USER\Software\Microsoft\Windows\CurrentVersion\Run`. This binary is a
    console program, so the app passes `--launcher` with itself and the value runs
    `"…\filebridge-desktop.exe" --start-engine`, which starts the service without a window and
    exits; without `--launcher` the value runs this binary and its console window stays open.
    It takes effect from the next login (nothing starts now) and removing it stops nothing.
    Task Manager's "Startup apps" can turn it off: `show` says `enabled: false`, installing
    turns it back on.
  - *macOS and Windows are unit-tested (the plist, the Run command line and its quoting, the
    launchd and Task Manager states) but not yet run on those systems.*
- **SSH agent under the unit:** the unit's process gets the user manager's environment, not your
  session's (on macOS launchd's, which has macOS's own agent; a socket under home, such as
  1Password's, is written into the plist's `EnvironmentVariables`; on Windows the Run value
  starts with the session's environment). If your `SSH_AUTH_SOCK` is a fixed per-user socket (`$XDG_RUNTIME_DIR/…`: GNOME
  Keyring, gcr, gpg-agent, `ssh-agent.socket`; or under home) the unit sets it as
  `Environment="SSH_AUTH_SOCK=%t/…"` (or `%h/…`): a path, never a key. If it moves at each login
  (`/tmp/ssh-…`) and `systemctl --user show-environment` lacks it, `install-user-unit` and
  `show-user-unit` warn (`UserUnit::agent` = `missing`, which the Settings confirmation says in
  pt/en): import it after each login, or give those destinations filebridge's own key.

## Tests
`tests/service.rs` runs the real binary with real rclone (skipped if rclone is missing):
- A second instance is refused.
- A send driven over the socket completes, with NFC names at the destination and events
  delivered.
- SIGTERM exits cleanly and removes the socket.
- SIGKILL mid-send, then a restart, resumes the job and finishes it byte for byte.

## Allowed dependencies
`core`, `ipc`, `host` (which wires `transport-rclone` and `integration-nextcloud`), `tokio`,
`clap`, `tracing-subscriber`, `anyhow`, `serde_json`; `winreg` on Windows (the Run key).
