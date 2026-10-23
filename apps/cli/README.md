# apps/cli

Headless CLI over the **same `core`** as the desktop app — for servers and scripts.
**Landed in:** Phase 0 (benchmark), Phase 1 Wave 2 (engine commands); published in Phase 3.

## Commands
```
filebridge profile add-local    --name N --root DIR [--windows]
filebridge profile add-sftp     --name N --host H [--port] [--user] [--key-file] [--known-hosts] --root /DIR
filebridge profile add-nextcloud --name N --host SERVER --nc-user U [--folder Backup] [--ctid] [--verbose]
filebridge profile add-immich --name N --server photos.example.com (--host H [--user] [--key-file] | --local)
                             --root DIR [--import-path /mnt/photos] [--library ID]   # ADR 0020
filebridge profile list | test <profile> | remove <profile>
filebridge profile trust-host --host H [--port 22] [--known-hosts F] [--fingerprint SHA256:…]
filebridge profile install-key <profile> | remove-access <profile> [--yes]
filebridge send <paths…> --to <profile> [--verify] [--concurrency 8] [--yes]
filebridge issues <job> [--rule R] [--all]
filebridge decide <job> <issue> accept|keep|skip|edit [--name N]
filebridge start <job> | resume [<job>] | cancel <job> | jobs | job <job>
filebridge settings [show] | settings set [--bwlimit 20M|off] [--window 22:00-07:00|off]
filebridge schedule add <paths…> --to <profile> --name N (--at HH:MM [--days mon,wed|daily] | --every-hours N [--at HH:MM]) [--into DIR] [--verify] [--disabled]
filebridge schedule list | remove <schedule> | run-now <schedule>
filebridge scan | fixture | bench        # Phase 0 benchmark tools (docs/runbooks/real-server-tests.md § 1)
```
- Job ids accept a unique prefix (the 8 characters `jobs` prints).
- The engine runs **in-process**: Ctrl-C pauses the job, and `kill -9` leaves it resumable
  with `filebridge resume`. The background service (Wave 3) removes the need to keep the
  command running.
- Data: `$FILEBRIDGE_DATA_DIR`, else the OS data dir (`~/.local/share/filebridge`).
  rclone: `--rclone` / `$FILEBRIDGE_RCLONE`, else `rclone` on PATH or `~/.local/bin/rclone`.
- `settings set` changes the app-wide bandwidth limit (rclone sizes: `20M` = 20 MiB/s, a plain
  number is KiB/s) and the daily send window, in local time (ADR 0010). A send started outside
  the window waits (`→ waiting`) and goes on its own when it opens.
- `schedule` manages scheduled sends (ADR 0019); `filebridge-service` runs them at their times,
  so `add` and `remove` need it stopped, like every command that changes state. `run-now` runs
  one here and follows it like `send`; each run sends only what is new or changed since the
  last, and keeps a changed file's older version at the destination under a dated name.
- Logs: `FILEBRIDGE_LOG=debug` (or `rclone=debug` for the sidecar's own log).

## Phase 0 scope (kept)
- Start `rclone rcd`, send a folder over SFTP with NFC-corrected names file by file, print
  progress, and measure files/s and MB/s against a plain `sync/copy` baseline.

## Later scope (draft)
```
filebridge profile add|list|test|remove-access
filebridge send <paths...> --to <profile> [--verify]
filebridge jobs [list|show|pause|resume|cancel|verify] <id>
filebridge repair --profile <p> <remote-path>
```

## Allowed dependencies
`core`, `transport-rclone`, `integration-nextcloud`, `clap`, `anyhow`, `tracing-subscriber`.

See [docs/09-roadmap.md](../../docs/09-roadmap.md).
