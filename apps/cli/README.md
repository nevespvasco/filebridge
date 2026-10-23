# apps/cli

Headless CLI over the **same `core`** as the desktop app — for servers and scripts.
**Landed in:** Phase 0 (benchmark), Phase 1 Wave 2 (engine commands); published in Phase 3.

## Install
For personal use it is installed from this checkout, not crates.io:
```bash
cargo install --path apps/cli --locked      # → ~/.cargo/bin/filebridge
filebridge --version                        # filebridge 0.0.1 (db1a3ef)
```
- `--locked` builds with the workspace's `Cargo.lock`, the versions the tests ran with.
- It needs rclone at run time (see `--rclone` below); `sidecars/RCLONE_VERSION` is the tested
  one.
- The desktop installers carry it too: the .deb and .rpm put it at `/usr/bin/filebridge` (the
  app itself is `filebridge-desktop`) with its man pages and bash/zsh/fish completions, and
  it finds the bundled `filebridge-rclone` next to it. On Windows and macOS it is installed
  beside the app, not on `PATH`.
- `--version` is the crate version and `git describe --tags --always` of the commit it was
  built from (`build.rs`, rerun when HEAD or a tag moves; no `--dirty`, which would go stale
  between commits). Built without the git history, set `FILEBRIDGE_GIT_DESCRIBE`, or it is
  the crate version alone.

### Completions and man pages
```bash
filebridge completions bash > ~/.local/share/bash-completion/completions/filebridge
filebridge completions zsh  > ~/.zfunc/_filebridge          # a folder on $fpath
filebridge completions fish > ~/.config/fish/completions/filebridge.fish
filebridge completions powershell >> $PROFILE                # also: elvish
filebridge man --dir ~/.local/share/man/man1                 # then: man filebridge-send
filebridge man | man -l -                                    # just the main page
```
`man --dir` writes one page per command and subcommand (`filebridge-profile-add-sftp.1`, …),
from the same text as `--help`, so they never drift.

## Commands
```
filebridge profile add-local    --name N --root DIR [--windows]
filebridge profile add-sftp     --name N --host H [--port] [--user] [--key-file] [--known-hosts] --root /DIR
filebridge profile add-nextcloud-webdav --name N --server cloud.example.com --user U [--folder Backup]
filebridge profile add-nextcloud --name N --host SERVER --nc-user U [--folder Backup] [--ctid] [--verbose]
filebridge profile add-immich --name N --server photos.example.com (--host H [--user] [--key-file] | --local)
                             --root DIR [--import-path /mnt/photos] [--library ID]   # ADR 0020
filebridge profile list | test <profile> | remove <profile>
filebridge profile trust-host --host H [--port 22] [--known-hosts F] [--fingerprint SHA256:…]
filebridge profile install-key <profile> | remove-access <profile> [--yes]
filebridge profile set-password <profile> [--remove]
filebridge send <paths…> --to <profile> [--into DIR] [--verify] [--concurrency 8] [--yes]
filebridge repair --to <profile> [--dir DIR] [--yes]
filebridge issues <job> [--rule R] [--all]
filebridge decide <job> <issue> accept|keep|skip|edit [--name N]
filebridge decide-all <job> accept|keep|skip [--rule R]
filebridge start <job> | resume [<job>] | cancel <job> | jobs | job <job>
filebridge report <job> [--format csv|json] [--only problems|renamed] [--dirs] [-o FILE]
filebridge settings [show] | settings set [--bwlimit 20M|off] [--window 22:00-07:00|off]
filebridge completions <shell> | man [--dir DIR]
filebridge scan | fixture | bench        # Phase 0 benchmark tools (docs/runbooks/real-server-tests.md § 1)
```
`filebridge --help` and `filebridge <command> --help` describe every option.
- Job ids accept a unique prefix (the 8 characters `jobs` prints).
- The engine runs **in-process**: Ctrl-C pauses the job, and `kill -9` leaves it resumable
  with `filebridge resume`. The background service (Wave 3) removes the need to keep the
  command running.
- Data: `$FILEBRIDGE_DATA_DIR`, else the OS data dir (`~/.local/share/filebridge`).
  rclone: `--rclone` / `$FILEBRIDGE_RCLONE`, else `rclone` on PATH or `~/.local/bin/rclone`.
- `settings set` changes the app-wide bandwidth limit (rclone sizes: `20M` = 20 MiB/s, a plain
  number is KiB/s) and the daily send window, in local time (ADR 0010). A send started outside
  the window waits (`→ waiting`) and goes on its own when it opens.
- Logs: `FILEBRIDGE_LOG=debug` (or `rclone=debug` for the sidecar's own log).

## Phase 0 scope (kept)
- Start `rclone rcd`, send a folder over SFTP with NFC-corrected names file by file, print
  progress, and measure files/s and MB/s against a plain `sync/copy` baseline.

## Allowed dependencies
`core`, `host`, `transport-rclone`, `integration-nextcloud`, `clap` (with `clap_complete` and
`clap_mangen`), `anyhow`, `tracing-subscriber`.

See [docs/09-roadmap.md](../../docs/09-roadmap.md).
