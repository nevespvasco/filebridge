# ADR 0009 — Direct mode also reaches Nextcloud on the SSH host itself

- Status: Accepted
- Date: 2026-10-22

## Context
[05 — Nextcloud](../05-integration-nextcloud.md) scoped v1 direct mode to **Nextcloud in a
Proxmox LXC**: SSH goes to the Proxmox node, and every command inside the container goes through
`pct exec <id> --`. The table in the same doc already said direct mode needs "SSH access to the
host **or the LXC**", but only the node path was built.

A common setup breaks this: the Nextcloud LXC (or VM) has its own SSH and is reached directly,
often over Tailscale, while the Proxmox node is not on the tailnet. The wizard then logs in, runs
`pct list`, gets exit 127 and stops with "`pct` not found". The same applies to Nextcloud on a VM
or a bare-metal server, where there is no Proxmox at all.

On the machine Nextcloud runs on, direct mode is simpler: there is no `pct exec`, no bind mount
to map (`datadirectory` is the path SFTP writes to), and no idmap (the owner is the web user's own
uid:gid, e.g. `33:33`).

## Decision
- `Runtime` gets a second variant, **`Runtime::Host`**: Nextcloud runs on the SSH host. Its
  "container" commands are the host commands themselves; `host_data_dir` equals `data_dir`
  (checked by `NextcloudDirectProfile::validate`). Saved settings: `"runtime": {"kind": "host"}`.
- A **`HostDetector`** (id `host`) sits after `LxcDetector` in `Nextcloud::detectors()`. It runs
  the same steps as the LXC detector, with the same helpers and step ids so the wizard shows them
  the same way: `host` (root, or passwordless `sudo -n`; needed for `chown` and `su`),
  `datadirectory` (Nextcloud's default `<install>/data` when `config.php` doesn't set it),
  `data-path`, `writable`, `occ`, `run-as`, `owner`.
- Detectors now report whether they **apply**. `LxcDetector` doesn't apply when `pct` is missing:
  `pct list` exits 127 and `/usr/sbin/pct` doesn't exist (the second check keeps a non-root user
  whose `PATH` lacks `/usr/sbin` on the LXC path, retried with `sudo -n`). The wizard runs
  detectors until one applies, and keeps the steps of those that didn't in front, so the Details
  panel shows why it moved on. A Proxmox node whose LXC detection fails is **not** searched for
  Nextcloud on the node itself, and a container chosen by id with no `pct` is still an error.
- The detection result names the detector (`Detection::detector`, and `detector` in the ipc
  `DetectNextcloudResponse`) so the UI and CLI can say which setup was found.
- Everything stays inside `crates/integration-nextcloud`; `core` and the contracts are unchanged.

## Consequences
- The wizard works when SSH reaches the Nextcloud LXC, VM or server directly; the UI's "Server"
  field takes either the Proxmox node or that machine.
- On the SSH host, the SSH user must be root or have passwordless sudo, as on the Proxmox node.
- *Amended 2026-10-23:* sudo is not enough on its own. SFTP writes the sent files **as the SSH
  user**, so that user must also be able to write to `<data>/<user>/files`, which belongs to the
  web server's user: in practice, log in as root. Both detectors check it as the SSH user itself
  (`test -w`, never through sudo) in a `writable` step right after `data-path`, so the wizard
  says so instead of the first send failing at preflight.
- Docker, AIO and Snap remain out of scope: they would be further detectors (and runtimes), added
  the same way.
- Rejected: a separate "connect to the container" toggle in the UI. Detection can tell the two
  setups apart by itself, and the user shouldn't need to know which one they have.
