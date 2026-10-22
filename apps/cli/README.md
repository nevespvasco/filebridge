# apps/cli

Headless CLI over the **same `core`** as the desktop app — for servers and scripts.
**Landed in:** Phase 0 (benchmark), Phase 1 Wave 2 (engine commands); published in Phase 3.

## Commands
```
filebridge profile add-local    --name N --root DIR [--windows]
filebridge profile add-sftp     --name N --host H [--port] [--user] [--key-file] [--known-hosts] --root /DIR
filebridge profile add-nextcloud --name N --host SERVER --nc-user U [--folder Backup] [--ctid] [--verbose]
filebridge profile list | test <profile> | remove <profile>
filebridge send <paths…> --to <profile> [--verify] [--concurrency 8] [--bwlimit 20M] [--yes]
filebridge issues <job> [--rule R] [--all]
filebridge decide <job> <issue> accept|keep|skip|edit [--name N]
filebridge start <job> | resume [<job>] | cancel <job> | jobs | job <job>
filebridge scan | fixture | bench        # Phase 0 benchmark tools
```
- Job ids accept a unique prefix (the 8 characters `jobs` prints).
- The engine runs **in-process**: Ctrl-C pauses the job, and `kill -9` leaves it resumable
  with `filebridge resume`. The background service (Wave 3) removes the need to keep the
  command running.
- Data: `$FILEBRIDGE_DATA_DIR`, else the OS data dir (`~/.local/share/filebridge`).
  rclone: `--rclone` / `$FILEBRIDGE_RCLONE`, else `rclone` on PATH or `~/.local/bin/rclone`.
- Logs: `FILEBRIDGE_LOG=debug` (or `rclone=debug` for the sidecar's own log).

## Phase 0 scope (kept)
- Start `rclone rcd`, send a folder over SFTP with NFC-corrected names file by file, print
  progress, and measure files/s and MB/s against a plain `sync/copy` baseline.

## Later scope (draft)
```
filebridge profile add|list|test|remove-access
filebridge send <paths...> --to <profile> [--verify] [--bwlimit 20M]
filebridge jobs [list|show|pause|resume|cancel|verify] <id>
filebridge repair --profile <p> <remote-path>
```

## Allowed dependencies
`core`, `transport-rclone`, `integration-nextcloud`, `clap`, `anyhow`, `tracing-subscriber`.

See [docs/09-roadmap.md](../../docs/09-roadmap.md).
