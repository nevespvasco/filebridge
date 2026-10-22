# ADR 0012 — filebridge's own SSH key, installed with `restrict`, removable

- Status: Accepted
- Date: 2026-10-22
- Refines: [07 — Security](../07-security-reliability.md#security) ("App-generated SSH key",
  "Restricted key in direct mode", "Remove access" wizard); [ADR 0004](0004-os-keyring-secrets.md)

## Context
The manual migration left a personal root SSH key behind on the server, and filebridge signs in
with the user's own key file or SSH agent. docs/07 proposed an app-generated key restricted with
`command="rrsync -wo …"`. That forced command cannot work here: transfers use rclone's SFTP, and
Nextcloud's direct mode also runs `occ`, `chown` and `pct` over SSH, so a key bound to one command
would break the scan.

Three ways were weighed: (A) filebridge's own key with `restrict`; (B) a dedicated `filebridge`
user with a `sudoers` rule limited to `occ`; (C) a forced-command gate script on the server that
allows only SFTP and the exact commands filebridge runs.

## Decision
**A.** For an SSH profile (SFTP, or Nextcloud direct mode), on the user's request:
- filebridge generates an Ed25519 key on this computer and keeps the private key **only** in the
  OS keyring (`filebridge/<profile-id>/ssh-key`). rclone receives it in memory (`key_pem`) and
  russh decodes it in memory; it is never written to a file.
- One line, `restrict ssh-ed25519 AAAA… filebridge-<profile-id>`, is added to the
  `~/.ssh/authorized_keys` of each SSH user the profile signs in as, using the access the profile
  has today. `restrict` turns off the interactive terminal, port, agent and X11 forwarding and
  `~/.ssh/rc`; SFTP and commands still work.
- The profile switches to the key only after a sign-in with it, and an rclone listing with it,
  have worked. Any failed step undoes the others (the line removed, the keyring entry deleted).
- **Remove access** deletes only the lines ending in `filebridge-<profile-id>` (rewritten in
  place, so the file keeps its owner and mode), deletes the keyring entry, puts the profile back on
  its key file or agent, and then tries the old key, held in memory, once more: the step succeeds
  only when the server refuses it. It is offered on the Destinations screen and on a finished
  transfer to that destination.

## Consequences
- The user's personal key is no longer used by filebridge and can be removed from the server. A
  stolen laptop exposes a key that can be revoked from anywhere by deleting one line.
- Least privilege is partial: the key still runs commands as the same user (often root for
  direct mode). B or C can come later on top of this; the install and remove flows stay the same.
- Nothing is installed or changed on the server beyond one line in `authorized_keys`.
- A server that reads keys from somewhere else (`AuthorizedKeysFile`, central key management)
  fails the sign-in check, and the install is undone; Remove access reports when the old key still
  signs in.
- Rejected for now: B (creates users and sudoers rules on the user's Proxmox host or container,
  more ways to break), C (fragile: every change to filebridge's commands needs the gate updated on
  each server).
