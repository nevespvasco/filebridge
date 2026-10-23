# ADR 0028 — The engine starts at login on Linux, macOS and Windows, from a per-user entry

- Status: Accepted
- Date: 2026-10-23

## Context
Settings → "Start the engine when I log in" was Linux-only: the `systemd --user` unit, which
`filebridge-service` writes and removes itself (`install-user-unit`, `uninstall-user-unit`,
`show-user-unit --json`) and the window only runs, after showing what it will write. On macOS
and Windows the switch was hidden and the engine ran only once the window had started it.

Each OS has its own per-user place for "run this at login", none of which needs administrator
rights:

- **macOS:** a LaunchAgent plist in `~/Library/LaunchAgents`, loaded by launchd at login.
- **Windows:** a value under `HKEY_CURRENT_USER\Software\Microsoft\Windows\CurrentVersion\Run`,
  or a Task Scheduler task.

`filebridge-service` is a console program. Started from the Run key or a logon task it opens a
console window that stays open for as long as it runs. A task can run hidden only as "whether
the user is logged on or not" (S4U), and such a process can't decrypt the user's Credential
Manager secrets, where the keyring keeps passwords and keys (ADR 0004). Hiding the console
from inside the service (`FreeConsole`) still flashes it at every login, and building it as a
GUI program breaks its terminal output (`status`, `show-user-unit`). The app, a GUI program,
already starts the service with no window (`DETACHED_PROCESS`).

## Decision
- One per-user **login entry** per OS, still written and removed only by the service binary
  through the same three commands, now on all three:
  - **Linux:** the `systemd --user` unit, unchanged.
  - **macOS:** `~/Library/LaunchAgents/app.filebridge.service.plist` (label
    `app.filebridge.service`): this binary with any explicit `--data-dir` / `--rclone`, then
    `run`; `RunAtLoad`, `KeepAlive` only on an unsuccessful exit and `ThrottleInterval` 5 (the
    unit's `Restart=on-failure`, `RestartSec=5`), its output in
    `~/Library/Logs/filebridge-service.log`. Installing clears a `launchctl disable` and, with
    `--no-start`, leaves loading to the next login; uninstalling runs `launchctl bootout`,
    which stops the engine it started. An SSH agent socket under home (1Password, a Homebrew
    agent) is written into `EnvironmentVariables`; macOS's own agent comes from launchd.
    Refused when the app runs from a translocated copy (opened where it was downloaded).
  - **Windows:** the Run value `filebridge-service`, not a scheduled task. The window passes
    itself as `--launcher`, so the value is `"…\filebridge-desktop.exe" --start-engine`: the
    app started with `--start-engine` (`filebridge_ipc::START_ENGINE_ARG`) starts the service
    as the window does, with no window, and exits. Installed from a terminal without
    `--launcher`, the value runs the service itself, console window and all. Task Manager's
    "Startup apps" can turn the value off (`…\Explorer\StartupApproved\Run`): read as
    `enabled: false`, and cleared when installing. Nothing starts when it is installed; the
    engine running now keeps running when it is removed.
- `UserUnit` (the JSON `show-user-unit` prints) gains **`kind`** (`systemd_unit`,
  `launch_agent`, `run_key`) and renames `systemd` to **`manager`**: the OS's login manager
  answers (`systemctl --user`; `launchctl` for the user's desktop session; always on Windows).
  The window words the confirmation by `kind` (pt and en) and shows the plist or the command
  line as it shows the unit file. `launchctl` not answering is `no_launchd`.
- Each module renders its entry on every OS, so the unit tests check the plist, the Run
  command line (Windows quoting) and the launchd and Task Manager parsing on Linux; only the
  calls that read and write the system (`launchctl`, the registry through `winreg`) are tied to
  the OS they run on.

## Consequences
- The switch shows on all three desktop OSes. On macOS and Windows it has not been run yet:
  the generated entries are unit-tested and the Windows registry code is checked to compile for
  `x86_64-pc-windows-msvc`, but nothing here exercised launchd or the registry.
- An update of the app that moves it (another folder, a renamed `.app`) leaves the entry
  pointing at the old path; Settings shows "Update" for it, as for the unit.
- The Windows entry depends on the app's binary staying where it was installed, and on the app
  keeping `--start-engine`.
- The window's replacement of an older service (apps/desktop README) is unchanged: on macOS a
  service started by launchd is stopped with SIGTERM, which is a successful exit, so launchd
  doesn't start it again and the window starts the new one; on Windows the user still quits an
  older service.
