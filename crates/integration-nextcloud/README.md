# crates/integration-nextcloud

First `Integration`: **Nextcloud**. **Lands in:** Phase 1 (direct mode), Phase 2 (WebDAV mode).

## Responsibility
- Two modes: **WebDAV** (default; app password; no scan needed — Phase 2) and **direct** (SFTP
  into the data folder; owner/permissions from the profile; `occ files:scan --path` on changed
  folders).
- Profile detection: pluggable `Detector`s (`Nextcloud::detectors()`, tried until one applies);
  `LxcDetector` for Nextcloud in a Proxmox LXC reached through the node, `HostDetector` for
  Nextcloud on the SSH host itself (its LXC or VM reached directly, bare metal; ADR 0009);
  Docker/AIO/Snap later.
- Name rules: `nextcloud-nfc`, `nextcloud-reserved` (`.htaccess`, `.part`, `.filepart`).
- Preflight and after-transfer for direct mode.

## Modules
| Module | What |
|---|---|
| `shell` | `RemoteShell` (`run`, streaming `run_lines`), `Output`, `SshTarget`, `ShellConnector` |
| `ssh` | `SshShell`/`SshConnector` over russh: host key checked against `known_hosts` (no "accept unknown"), auth with an unencrypted key file or the SSH agent |
| `fake` | `FakeShell` (answers keyed by exact command string, records what ran) and `FakeConnector`, public for `core`/CLI tests |
| `quote` | POSIX `sh` quoting; every command is built with it |
| `profile` | `NextcloudSettings` (tagged `mode: "direct"`), `NextcloudDirectProfile`, command builders (`host_cmd`, `container_cmd`, `occ_cmd`) |
| `detect` | `Detector`, `LxcDetector`, `HostDetector`, `DetectInput`, `Detection` (steps with the commands they ran; whether the detector applies) |
| `parse` | `pct list`, `pct config`, `config.php`, `id`, `stat`, `df`, `occ status`, `occ files:scan` output |
| `direct` | preflight, owner/mode fix, sent-only scan plan, scan with progress |
| `rules` | `NextcloudNfc`, `NextcloudReserved` |
| `setup` | The profile wizard, `Nextcloud::set_up(req, transport)`: connect with the integration's own `ShellConnector`, run each detector, build the destination (SFTP to the SSH host, rooted at the user's files folder) and settings. `check_direct` is what every saved profile must pass (valid settings, same host for SFTP and SSH, root inside the files folder); `Integration::check_profile` runs it on every save |

## Direct mode
**Commands.** The SSH host is the Proxmox node (`Runtime::ProxmoxLxc`) or the machine Nextcloud
runs on (`Runtime::Host`). Host commands run as the SSH user, or through `sudo -n` when detection
found that `pct` (or, on the Nextcloud machine, a non-root user) needs it. Container commands are
`pct exec <id> -- argv…` (no shell inside); for `Runtime::Host` they are the host commands
themselves. `occ` runs as the web user through `sudo -n -u www-data php occ …`, or
`su -s /bin/bash www-data -c '<quoted line>'` when there is no sudo where Nextcloud runs (quoted
twice: once for `su -c`, once for the SSH command line).

**Detection, Proxmox node** (`LxcDetector`; each step a `Check` plus the commands it ran, for the
Details panel):
1. `pct-list` — `pct list` (retried with `sudo -n`); picks the one running container named like
   Nextcloud, or `DetectInput::ctid`. Exit 127 with no `/usr/sbin/pct` means this is not a
   Proxmox node: the step passes, the detector reports it doesn't apply, and the wizard goes on
   to `HostDetector` (unless a container was chosen by id, which fails).
2. `pct-config` — `pct config <id>`: `mpN` mounts (host paths, or storage volumes resolved with
   `pvesm path`), `unprivileged`, `lxc.idmap`.
3. `datadirectory` — finds `config/config.php` (`ls -d` over the usual install dirs, or
   `DetectInput::install_dir`), reads **only** the `datadirectory` line (`grep -F`; the file holds
   the DB password), and maps it to the host through the covering bind mount. No mount covering
   it fails with an explanation (data on the container's own disk).
   `data-path` — the container path, the host path and `<host>/<user>/files` exist.
4. `occ` — `occ` exists; the web user is the owner of `config.php` (default `www-data`).
5. `run-as` — `command -v sudo` in the container chooses sudo or su; `occ status --output=json`
   proves it (maintenance mode is a warning here).
6. `owner` — `id <web user>` mapped through the idmap (default unprivileged: `100000 + 33` =
   `100033`); cross-checked against `stat -c %u:%g` of the host data dir (mismatch = warning).

**Detection, Nextcloud on the SSH host** (`HostDetector`; same helpers and step ids):
1. `host` — `id -u` is 0, or `sudo -n true` works (setting the owner needs root).
2. `datadirectory` — as above, without the mount mapping; no `datadirectory` line means
   Nextcloud's default `<install>/data`. `data-path` — the data dir and `<data>/<user>/files` exist.
3. `occ`, `run-as` — as above, on the host.
4. `owner` — `id <web user>` as is (no idmap), cross-checked with `stat -c %u:%g`.

Stdout is never kept in the detection result.

**Preflight:** SSH connects; `<host data>/<user>/files` exists; the destination root (or the files
folder, if the root doesn't exist yet) is writable by the SSH user itself; `occ status` is
installed, not in maintenance, no pending DB upgrade; free bytes from `df -Pk`.

**After transfer** (`changed` is relative to the destination root, which must be inside
`<datadir>/<user>/files`, host or container view):
1. Unless the transport has `CHOWN`/`CHMOD`: `chown -h uid:gid` and
   `find … -maxdepth 0 -type d -exec chmod 750 {} + -o -type f -exec chmod 640 {} +` on the changed
   paths and the directories between them and `files/` (new directories need the owner too);
   batched under 96 KiB per command line. Failure is a fatal error (Nextcloud couldn't read the
   files).
2. Scan only what was sent, never files that were already there. Starting at the destination
   root, one remote command per level counts the files in each folder (`find -type f | head -n
   <sent+1> | wc -l`, so the count stops early). A folder holding only sent files gets one
   recursive scan. In a folder that also holds other files, the sent files directly in it are
   scanned one by one (above 16, one `--shallow` scan of the folder instead), and the subfolders
   holding sent files are decided the same way.
3. `occ files:scan -v [--shallow] --path=<user>/files/<folder or file>` per target. Output is streamed: `File` lines
   drive `StepProgress.items` (every 100), 15 s of silence sends `Waiting("nextcloud.waiting-for-disk")`,
   the summary table gives the final count.
4. `Entry "…" will not be accessible due to incompatible encoding` → a
   `nextcloud.incompatible-encoding` problem with `RepairDestination`; scan errors and non-zero
   exits → `nextcloud.scan-error` / `nextcloud.scan-failed` problems with `Retry`.

## Not yet
- Encrypted key files and keys from the keyring (docs/07: app-generated key) — needs `core` to
  pass key material; for now use the agent or an unencrypted key file.
- SSH straight into the container (no `pct`), with the scoped `sudoers` rule from docs/07: a
  second `Runtime` variant.
- WebDAV mode (Phase 2).

## Allowed dependencies
`integration-api`, `transport-api`, `names`, `russh`, `serde`, `serde_json`, `tokio`, `tracing`,
`async-trait`; `reqwest` when WebDAV lands.

See [docs/05-integration-nextcloud.md](../../docs/05-integration-nextcloud.md).
