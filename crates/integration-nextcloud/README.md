# crates/integration-nextcloud

First `Integration`: **Nextcloud**. **Lands in:** Phase 1 (direct mode), Phase 2 (WebDAV mode).

## Responsibility
- Two modes: **WebDAV** (default; app password; no scan needed — Phase 2) and **direct** (SFTP
  into the data folder; owner/permissions from the profile; `occ files:scan --path` on changed
  folders).
- Profile detection: pluggable `Detector`s (`Nextcloud::detectors()`, tried until one applies);
  `LxcDetector` for Nextcloud in a Proxmox LXC reached through the node, `HostDetector` for
  Nextcloud on the SSH host itself (its LXC or VM reached directly, bare metal; ADR 0009);
  `DockerDetector` (official image, AIO) and `SnapDetector`, tried between the two.
- Name rules: `nextcloud-nfc`, `nextcloud-reserved` (`.htaccess`, `.part`, `.filepart`).
- Preflight and after-transfer for direct mode.

## Modules
| Module | What |
|---|---|
| `shell` | `RemoteShell` (`run`, streaming `run_lines`), `Output`, `SshTarget`, `ShellConnector` |
| `known_hosts` | The file's default path, the keys recorded for a host (with real line numbers), where a key stands (`Known`/`Unknown`/`Changed`, any other key on record counts as changed), the host key algorithms to offer, and `append`: adds a plain line at the end, only for a host with no key on record, creating `~/.ssh` 0700 and the file 0600; refuses host names that would widen the line and relative paths. |
| `ssh` | `SshShell`/`SshConnector` over russh: host key checked against `known_hosts`, offering only the key types on record (a connection never accepts an unknown host; `scan_host_key` and `trust_host_key` let the wizards add one after the user confirms its fingerprint, never a changed one: ADR 0015), auth with filebridge's own key read through the host's `SecretResolver` (`SshTarget::key`, ADR 0012), else an unencrypted key file or the SSH agent; `key_accepted` and `generate_key` for the key flows; a command dropped before it ends closes its channel. Also used by the host for plain SFTP profiles. |
| `webdav` | WebDAV mode: address parsing with the `url` crate (`parse_address`, `server_url`: HTTPS except on loopback, no credentials in it, the user id from a pasted WebDAV address), `dav_url`, the profile check, preflight and after-transfer (nothing to do) |
| `fake` | `FakeShell` (answers keyed by exact command string, records what ran) and `FakeConnector`, public for `core`/CLI tests |
| `quote` | POSIX `sh` quoting; every command is built with it |
| `profile` | `NextcloudSettings` (tagged `mode: "direct"`), `NextcloudDirectProfile`, command builders (`host_cmd`, `container_cmd`, `occ_cmd`) |
| `detect` | `Detector`, `LxcDetector`, `DockerDetector`, `SnapDetector`, `HostDetector`, `DetectInput`, `Detection` (steps with the commands they ran; whether the detector applies; whether a step failed for want of rights; a dedicated user's server setup) |
| `parse` | `pct list`, `pct config`, `docker ps`, `docker inspect` mounts, `config.php`, `id`, `stat`, `df`, `occ status`, `occ files:scan` output |
| `dedicated` | The server setup for a dedicated user (ADR 0024): `useradd`/`usermod`, `chmod -R g+rwX`, the sudoers line (`sudoers_rule`, escaped; none when a name can't be held), for the user to run |
| `direct` | preflight, owner/mode fix, sent-only scan plan, scan with progress |
| `rules` | `NextcloudNfc`, `NextcloudReserved` |
| `setup` | `Nextcloud::advise(req)`: the wizard's mode recommendation (ADR 0024), detection without saving → `Advice { mode, reason, message, detection }`. The profile wizard, `Nextcloud::set_up(req, transport)`: connect with the integration's own `ShellConnector`, run each detector, build the destination (SFTP to the SSH host, rooted at the user's files folder) and settings. `check_direct` is what every saved profile must pass (valid settings, same host for SFTP and SSH, root inside the files folder); `Integration::check_profile` runs it on every save |

## Direct mode
**Commands.** The SSH host is the Proxmox node (`Runtime::ProxmoxLxc`) or the machine Nextcloud
runs on (`Runtime::Host`). Host commands run as the SSH user, or through `sudo -n` when detection
found that `pct` (or, on the Nextcloud machine, a non-root user) needs it. Container commands are
`pct exec <id> -- argv…` (no shell inside); for `Runtime::Host` they are the host commands
themselves. `occ` runs as the web user through `sudo -n -u www-data php occ …`, or
`su -s /bin/bash www-data -c '<quoted line>'` when there is no sudo where Nextcloud runs (quoted
twice: once for `su -c`, once for the SSH command line).

**Long `occ` commands die with the connection** (`occ_cmd_stop_on_eof`, used for `files:scan`).
sshd signals nothing when a client goes away, and PHP ignores SIGPIPE, so a scan whose service
was killed, restarted or cancelled kept running on the server (and a resumed job's new scan then
collided with it on Nextcloud's scan lock). The `occ` line is wrapped in a small `sh` script, run
as the web user right next to `php` (`su … -c '<script>'`, or `sudo -n -u www-data sh -c
'<script>'`): `php occ …` runs in the background reading `/dev/null`, and a watcher reads the
script's stdin — the SSH channel, which filebridge never writes to — and sends `php` SIGTERM on
EOF, i.e. when sshd closes the channel. stdin passes untouched through `sudo`, `pct exec` and
`su` (no PTY is requested: a PTY would mix stderr into stdout, make `occ` format its output for
a terminal, and its SIGHUP would have to be relayed by every layer). The watcher starts only when stdin is a pipe or
socket, so the script with any other stdin just runs `occ` to the end. `occ` traps SIGTERM when
PHP has `pcntl` and stops at the next file. On the client side, dropping a running command (job
cancelled) closes its channel even if the connection stays open. Not covered: a connection that
dies without the server noticing (network gone, no `ClientAliveInterval`) keeps the scan running
until sshd's TCP keepalive gives up.

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
1. Unless the transport has `CHOWN`/`CHMOD`: the sent files get `chown -h uid:gid` and
   `find … -maxdepth 0 -type f -exec chmod 640 {} +`. The folders between them and `files/` change
   only if the transfer created them: SFTP makes folders as the SSH user (`id -u`), Nextcloud's
   own belong to the web user, so `find … -maxdepth 0 -type d -user <ssh uid>` picks the new ones
   (mode `750` first, then the owner). Folders that were there keep their owner and mode. The
   owner is the server-side record of what is done, so a resumed job or a retried step fixes the
   same folders and nothing it already fixed; nothing is kept in the job. When the SSH user is
   the owner the profile wants, new folders can't be told apart and are left as they are.
   Batched under 96 KiB per command line. Failure is a fatal error (Nextcloud couldn't read the
   files).
2. Scan only what was sent, never files that were already there. Starting at the destination
   root, one remote command per level counts the files in each folder (`find -type f | head -n
   <sent+1> | wc -l`, so the count stops early). A folder holding only sent files gets one
   recursive scan. In a folder that also holds other files, the sent files directly in it are
   scanned one by one (above 16, one `--shallow` scan of the folder instead), and the subfolders
   holding sent files are decided the same way.
3. `occ files:scan -v [--shallow] --path=<user>/files/<folder or file>` per target, stopped if the
   connection closes (above). Output is streamed: `File` lines
   drive `StepProgress.items` (every 100), 15 s of silence sends `Waiting("nextcloud.waiting-for-disk")`,
   the summary table gives the final count.
4. `Entry "…" will not be accessible due to incompatible encoding` → a
   `nextcloud.incompatible-encoding` problem with `RepairDestination`; scan errors and non-zero
   exits → `nextcloud.scan-error` / `nextcloud.scan-failed` problems with `Retry`.

**Detection, Docker** (`DockerDetector`, `Runtime::Docker { container }`):
1. `docker` — `docker ps --format '{{.Names}} {{.Image}}'` (retried with `sudo -n`). Not
   installed (127): the detector doesn't apply, nothing said. Unusable: a warning, and the next
   detector looks. The one running container named `nextcloud-aio-nextcloud` (AIO) or from an
   image named `nextcloud`; none → doesn't apply; several → fails naming them.
2. `mounts` — `docker inspect --format '{{json .Mounts}}' <container>`.
3. `datadirectory` — `config.php` in `/var/www/html` (official, AIO), `/app/www/public` or
   `/config/www/nextcloud` (linuxserver.io); `datadirectory` (or `<install>/data`) mapped to the
   host through the mount holding it (longest `Destination`). None → fails: the data is in the
   container's own layer. `data-path`, `writable` as for the LXC.
4. `occ`, `run-as` — `occ` and the owner of `config.php` in the container; `docker exec -u
   <web user> <container> php occ status --output=json`.
5. `owner` — `id <web user>` in the container, the same numbers on the host (no user namespace),
   cross-checked with `stat` (AIO's `33:0` warns).
Container commands are `docker exec <container> argv…`; long `occ` commands
`docker exec -i -u <web user> <container> sh -c '<watcher>'` (`-i` hands the SSH channel in).

**Detection, the snap** (`SnapDetector`, `Runtime::Snap`): `snap` — `snap list nextcloud`
(missing: doesn't apply); `host` — root or `sudo -n`; `datadirectory` — from the snap's
`config.php`, else `/var/snap/nextcloud/common/nextcloud/data`; `data-path`, `writable`;
`run-as` — `/snap/bin/nextcloud.occ status --output=json`; `owner` — the data folder's own
(`stat`, `0:0` as the snap runs Nextcloud as root).

**Detection, a dedicated user** (`HostDetector` with `DetectInput::dedicated_user`, ADR 0024;
`LxcDetector` doesn't apply, `pct` needs root):
1. `host` — `id -u` is not 0; `id -G` gives its groups.
2. `occ` — `occ` in the install dir (`ls -d` over the usual ones: `config/` may not be readable
   yet), its owner as the web user, `command -v php` (a rule names php by its full path), `id`
   and `id -gn` of the web user.
3. `group` — the SSH user is in the web user's group (else the setup's `usermod`).
4. `datadirectory` — read through the group; a `grep` error (exit 2) fails instead of being taken
   for "no line". `data-path`, then `writable` (else the setup's `chmod -R g+rwX`).
5. `run-as` — `sudo -n -u <web> <php> occ status --output=json` through the rule (else the
   setup's sudoers line).
6. `owner` — as for the host.
`Detection::server_setup` (`dedicated::server_setup`) holds the commands for the user to run as
root, built from what was found (placeholders for what wasn't); filebridge never runs them. The
profile: `Access::Dedicated`, `RunAs::SudoRule`, no host sudo, modes `770`/`660`. After a
transfer it uses `chgrp -h -- <gid>` instead of `chown`; long `occ` commands run as
`sh -c '<watcher around: sudo -n -u <web> php occ …>'`.

## Not yet
- Encrypted key files (load them into the SSH agent instead).
- A dedicated user on a Proxmox node (`pct` needs root) or in Docker (the `docker` group is root).

## Allowed dependencies
`integration-api`, `transport-api`, `names`, `russh`, `serde`, `serde_json`, `tokio`, `tracing`,
`async-trait`, `getrandom` (new keys), `zeroize`, `url` (WebDAV addresses). Never
`filebridge-secrets`: the SSH connector reads a profile's own key through the `SecretResolver`
the host injects.

See [docs/05-integration-nextcloud.md](../../docs/05-integration-nextcloud.md).
