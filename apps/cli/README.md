# apps/cli

Headless CLI over the **same `core`** as the desktop app — for servers and scripts.
**Lands in:** Phase 0 (prototype), published in Phase 3.

## Phase 0 scope
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
