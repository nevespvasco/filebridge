# 05 — Nextcloud integration

Crate: `crates/integration-nextcloud`. Implements `Integration` and contributes Nextcloud's
name rules.

## Two modes

Chosen automatically from the size of the send and the access available. **WebDAV is the
default** because it needs no server access; **direct mode** is for large loads on the local
network (like the 500 GB initial migration).

| | WebDAV mode (default) | Direct mode (server) |
|---|---|---|
| How it sends | Nextcloud API, chunked upload (rclone `webdav` backend, vendor `nextcloud`) | SFTP into the data folder, as done with rsync (rclone `sftp` backend) |
| Needs | URL, user and app password | SSH access to the host or the LXC |
| Indexing | Automatic, no scan | `occ files:scan --path` only on the sent files and folders |
| Owner and permissions | Handled by Nextcloud | Applied by the profile (e.g. `100033:100033`, `750/640`) |
| Speed | Slower (HTTP overhead per file) | Limited by disk and network |
| Works away from home | Yes | Only on LAN or VPN |
| Recommended for | Up to tens of GB, daily use | Large initial migrations |

### As built (Phase 2)

- **WebDAV mode** (`crates/integration-nextcloud/src/webdav.rs`): `Backend::Webdav { url, user,
  password, vendor: "nextcloud" }` through rclone's `webdav` backend; settings
  `{ "mode": "webdav", "server", "user" }`. The wizard (`add_nextcloud_webdav`, the window's
  "Over the internet (WebDAV)" form, `filebridge profile add-nextcloud-webdav`) normalises what
  is typed (`cloud.example.com`, or a WebDAV address copied from Nextcloud) to
  `https://…/remote.php/dav/files/<id>/` — plain `http` only on this machine (`localhost` or a
  loopback IP as parsed by the `url` crate, never a name like `127.example.org`); an address with
  a user or password in it is refused, as are bad ports and IPv6 literals. `<id>` is the login,
  or the user id from a pasted WebDAV address (`…/dav/files/<id>/`) when it differs, as with
  email or LDAP logins (settings `user_id`); a missing user folder suggests pasting it. It keeps the app
  password in the OS keyring under a fresh profile id, signs in (lists the user's files), looks
  at the folder (missing = created by the first send) and the quota, and saves the profile only
  when sign-in works; a refused sign-in removes the password again.
- Preflight adds nothing to the transport's (sign-in, quota: `transport-nextcloud` reads the
  user's WebDAV quota properties and reports them when tighter than rclone's numbers, so the
  "may run out of space" warning during a send uses the quota); after the transfer there is
  nothing to do. In direct mode the quota comes from `occ user:info` in the integration's
  preflight, asked once per transfer stage (D03). Verification hashes by download (Nextcloud can't give SHA-256), so it costs a second
  pass over the data.
- A wrong app password is an `Auth` error (WebDAV's 401), not a retry; the transfer stops at the
  first one instead of signing in once per file (Nextcloud's brute-force protection counts
  each). HTTP statuses are read from their token (`401 Unauthorized`), never from free text:
  `403` (a blocked name) and `413` are `Other`, `507` (no space, a quota reached) is `Full`, which
  pauses the job until space is freed ([ADR 0021](adr/0021-full-destination-pauses-the-job.md)),
  other `5xx`, `423` and `429` are `Transient`.
- Removing the app password of a WebDAV profile is refused: the profile needs it.
- Speed depends on the password being an **app password**. Nextcloud checks an account password
  with a slow hash on every request (~175 ms against ~43 ms for an app password on Nextcloud 31),
  and rclone makes three to four requests per file: 1,200 small files took 371 s with the account
  password and 40 s with an app password (crates/transport-rclone/README.md).
- Small files (up to 4 MiB) go by Nextcloud's **bulk upload**, 100 per request, when the server
  offers it (`transport-nextcloud`, [ADR 0016](adr/0016-nextcloud-bulk-upload.md)): the same
  1,200 files in 7.6 s, and 14 s even with the account password. Bigger files, and anything the
  bulk upload doesn't take, go through rclone.
- Mode auto-selection is built as advice in the wizard (below); the send-size threshold is not.

### Mode auto-selection

The wizard recommends a mode from the server itself ([ADR 0024](adr/0024-nextcloud-mode-advice-and-occ-user.md)):
"Not sure which to pick? Let filebridge check the server" asks for the SSH server, port and user
and the Nextcloud user, and `probe_nextcloud` (`Nextcloud::advise`) runs direct mode's detection
without saving anything.

- Every step passes → **direct mode**: SSH reaches the machine Nextcloud runs on with the rights
  it needs.
- SSH doesn't connect, or the server's key isn't trusted → **WebDAV**.
- Nextcloud is there but the SSH user lacks root (or `sudo -n`) or can't write the files folder
  → **WebDAV**, saying which right is missing.
- No usable Nextcloud found over SSH → **WebDAV**.

The window moves the mode control to the recommendation, explains it (pt and en, with the step's
own message), fills the chosen form with what was typed, and the user can still pick the other
mode. Not built: suggesting direct mode only above a send size (20 GB or 50 000 files).

## Profile detection wizard

The Nextcloud profile wizard detects the setup by itself:

1. **Ask for the URL and an app password**, and test via WebDAV (`PROPFIND` on
   `/remote.php/dav/files/<user>/`). Store the app password in the keyring.
2. **If the user enables direct mode**, connect over SSH to the server — the Proxmox node, or the
   machine Nextcloud runs on — and run `pct list`.
   - **`pct` exists** (Proxmox node): run `pct config <id>` to find the LXC and its bind mounts
     (`mpN:` lines). Container commands go through `pct exec <id> --`.
   - **No `pct`** (exit 127 and no `/usr/sbin/pct`): Nextcloud runs on the SSH host itself — its
     LXC or VM reached directly (e.g. over Tailscale), or a bare-metal server. Commands run there
     as they are; the SSH user must be root or have passwordless `sudo -n`. See
     [ADR 0009](adr/0009-nextcloud-on-the-ssh-host.md).
   - Either way, SFTP writes the files **as the SSH user**, so that user (not sudo) must be able
     to write to the user's `files` folder: the `writable` step checks it after `data-path`. In
     practice the SSH user is root.
3. **Read `datadirectory` from `config.php`** and confirm the path exists (inside the container and
   mapped to the host via the bind mount). This avoids the original "No such file or directory"
   error, when Nextcloud was still using the container's internal disk. On the SSH host itself
   there is no mapping: `datadirectory` (or Nextcloud's default `<install>/data`) is the path
   SFTP writes to.
4. **Find the `occ` path** and whether the container (or machine) has `sudo` (the real one didn't)
   or needs `su -s /bin/bash www-data -c '…'`.
5. **Compute the mapped UID** (`100000 + 33` = `100033`) from the LXC `idmap` (default unprivileged
   offset 100000 unless `lxc.idmap` lines say otherwise) and the `www-data` UID inside the container.
   On the SSH host itself the owner is the web user's own uid:gid (e.g. `33:33`).
6. **Save everything in a reusable profile** (`profiles` table; secrets as keyring references).

Each step shows ✓/✗ and an actionable message on
failure. When the host has no `pct`, the "Containers" step says so and the steps for Nextcloud on
the host follow it, so the panel shows which setup was found.

### Setups supported

v1 targets **Nextcloud in a Proxmox LXC** (the author's setup), reached either through the Proxmox
node (`LxcDetector`, `Runtime::ProxmoxLxc`) or directly over SSH, and more generally **Nextcloud
on the SSH host** — an LXC, a VM or a bare-metal server with a package or tarball install
(`HostDetector`, `Runtime::Host`; [ADR 0009](adr/0009-nextcloud-on-the-ssh-host.md)), and
**Docker, AIO and the snap** on the SSH host. Detection is a list of `Detector`s tried in order
until one applies — Proxmox node, Docker, snap, then Nextcloud installed on the host — so each
of these was added without changing the wizard:

| Setup | Found by | `occ` | Data folder (container → host) | Owner of sent files |
|---|---|---|---|---|
| Proxmox LXC | `pct list` | `pct exec <id> -- su/sudo -u www-data php occ` | `datadirectory`, through the `mpN` bind mount | the container's web user through the idmap (`100033`) |
| Installed on the host | (the fallback) | `su`/`sudo -u www-data php occ` | `datadirectory` (or `<install>/data`), same path | the web user (`33:33`) |
| Docker, official image | `docker ps`: an image named `nextcloud` | `docker exec -u www-data <container> php /var/www/html/occ` | `datadirectory` (`/var/www/html/data`), through the volume or bind mount that holds it (`docker inspect` `.Mounts`, longest match) | the web user's ids in the container (`33:33`; no user namespace) |
| Nextcloud AIO | `docker ps`: the container `nextcloud-aio-nextcloud` (AIO's other containers are left out) | as Docker | `/mnt/ncdata` → the `nextcloud_aio_nextcloud_data` volume (or `NEXTCLOUD_DATADIR`'s bind mount) | as Docker; AIO's data folder is `33:0`, so the owner check warns |
| Snap | `snap list nextcloud` | `/snap/bin/nextcloud.occ`, as root | `datadirectory` from `/var/snap/nextcloud/current/nextcloud/config/config.php`, else `/var/snap/nextcloud/common/nextcloud/data` | the data folder's own owner (`0:0`: the snap runs Nextcloud as root) |

Docker needs root or the `docker` group (`docker ps` is retried with `sudo -n`); Docker present
but unusable is a warning and detection looks further. Long `occ` commands in Docker run as
`docker exec -i … sh -c '<watcher>'`, so the scan still dies with the SSH connection. Not yet:
choosing between several Nextcloud containers on one host (detection stops and names them),
and Docker with user-namespace remapping (the owner check warns).

The wizard lives in the plugin (`Nextcloud::set_up`): it detects over the integration's own
`ShellConnector` (so it is tested with the fake shell) and builds the whole profile: SFTP to the
same host as SSH, rooted at `<user>/files/<folder>`. The hosts only save it. Every saved
profile is checked by `Integration::check_profile` (same SFTP and SSH host, root inside the
files folder, valid settings), whether the wizard, the CLI or the UI saves it.

## Name rules contributed

- `nextcloud-nfc` — names must be NFC (Nextcloud rejects NFD).
- `nextcloud-reserved` — `.htaccess`, `.part` suffix and other names blocked by the server.
- Uses the general `forbidden-chars`/`trailing` only if the server is configured with Windows
  compatibility.

## Preflight

- WebDAV: auth OK, quota (`quota-available-bytes`), target folder exists or can be created.
- Direct: SSH OK, data path exists and is writable by the dedicated user, `occ status` works,
  free disk on the data volume (`df`).

## After transfer (scan)

- **WebDAV mode:** nothing to do — Nextcloud indexes on upload.
- **Direct mode:**
  1. Apply owner/mode if the transport couldn't (`100033:100033`, dirs `750`, files `640`) — only
     on the files this job sent and the folders the transfer created. A folder that was already
     there keeps its owner and mode. New folders are the parents still owned by the SSH user
     (SFTP creates them as that user; Nextcloud's own belong to the web user), and each gets its
     mode before its owner, so a resumed job finds exactly the ones still to fix.
  2. Scan **only the sent files**, never what was already in the destination: a folder that holds
     nothing but sent files is scanned recursively; in a folder that also holds other files, the
     sent files are scanned one by one (many of them: `--shallow` on the folder) and its
     subfolders are decided the same way. File counts on the server tell the two apart.
  3. Run `occ files:scan --path="<user>/files/<folder or file>"` for each, **showing progress**
     parsed from `occ` output ("Indexing in Nextcloud: 3 / 5", files scanned). The scan **dies
     with the SSH connection**: if the job is cancelled or the service is killed or restarted,
     `occ` gets SIGTERM instead of scanning on as an orphan (which also kept a resumed job's new
     scan from hitting "Another process is already scanning").
  4. Treat `incompatible encoding` warnings as **visible errors** with a "Repair destination"
     button, instead of leaving them lost in the terminal.
  5. On slow HDDs show "waiting for disk" instead of looking stuck.

## Security in direct mode

See [07 — Security](07-security-reliability.md#security) and
[ADR 0012](adr/0012-own-ssh-key-with-restrict.md): filebridge's own key (in the keyring, installed
with `restrict` for both the SFTP and the SSH user), and "Remove access". A **dedicated user** instead of root is
an opt-in ([ADR 0024](adr/0024-nextcloud-mode-advice-and-occ-user.md)): on the machine Nextcloud
runs on, in the web server's group (it hands what it sends to that group with `chgrp`, modes
`770`/`660`, so it needs no root), with one `sudoers` line that allows only the `occ` commands
direct mode runs, as the web user. Detection as that user writes out the server setup
(`usermod -a -G`, `chmod -R g+rwX` on the files folder, the sudoers line with `visudo -cf`) for
the user to read and run; filebridge never runs it. A step that finds a right missing (`group`,
`datadirectory`, `writable`, `run-as`) fails and names the setup step to run.
