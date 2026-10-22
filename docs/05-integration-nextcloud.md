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

### Mode auto-selection

- Profile has only WebDAV credentials → WebDAV.
- Profile has direct mode configured **and** the server is reachable (preflight) **and** the send
  is above a threshold (default 20 GB or 50 000 files, configurable) → suggest direct mode.
- The user can always override in step 4 of the wizard.

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

Each step shows ✓/✗ with the command it ran in the "Details" panel and an actionable message on
failure. When the host has no `pct`, the "Containers" step says so and the steps for Nextcloud on
the host follow it, so the panel shows which setup was found.

### Setups supported

v1 targets **Nextcloud in a Proxmox LXC** (the author's setup), reached either through the Proxmox
node (`LxcDetector`, `Runtime::ProxmoxLxc`) or directly over SSH, and more generally **Nextcloud
on the SSH host** — an LXC, a VM or a bare-metal server with a package or tarball install
(`HostDetector`, `Runtime::Host`; [ADR 0009](adr/0009-nextcloud-on-the-ssh-host.md)). Docker, AIO
and Snap differ in `occ` invocation and data paths; the detector grows case by case (see
[10 — Risks](10-risks-and-open-decisions.md)). Detection is structured as a list of `Detector`s,
tried until one applies, so adding `DockerDetector` doesn't change the wizard.

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
  1. Apply owner/mode if the transport couldn't (`chown -R 100033:100033`, dirs `750`, files `640`)
     — only on changed paths.
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

See [07 — Security](07-security-reliability.md#security): app-generated key, dedicated user,
`authorized_keys` with `command=` + `rrsync` + `restrict`, `sudoers` rule limited to `occ`, and the
"Remove access" wizard.
