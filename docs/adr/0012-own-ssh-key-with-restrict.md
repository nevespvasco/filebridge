# ADR 0012 — filebridge's own SSH key, installed with `restrict`, removable

- Status: Accepted
- Date: 2026-10-22 (revised the same day after review: safe rewrite of `authorized_keys`,
  proof before the profile changes)
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
  have worked. Any failed step (saving the profile included) undoes the others (the line
  removed, the keyring entry deleted), and the report says what the undo did for each server
  and the keyring: a failed undo names the line left behind instead of claiming that nothing
  changed.
- **Remove access** deletes only the lines whose last word is `filebridge-<profile-id>` (also
  when they end in `\r` or spaces), then tries the old key, held in memory, once more. Only when
  no server accepts it is the profile put back on its key file, SSH agent or password, and then
  the keyring entry deleted. A server that takes the key as a first factor and asks for a
  password next (`AuthenticationMethods publickey,password`, "partial success") accepts it; a server that still accepts it leaves the profile on the key and
  the step failed. It is offered on the Destinations screen, on a finished transfer to that
  destination, and from the delete-destination dialog when the destination has the key.
- `authorized_keys` is only ever changed by scripts that leave it as it was unless every step
  worked: bytes are compared as bytes (`LC_ALL=C grep -a`, so a Latin-1 comment or a NUL byte
  can't make GNU grep drop lines or answer "binary file matches"; a `grep` without `-a`, as on
  some BusyBox builds, is detected and used without it), a `grep` error stops it, and
  the filtered copy (made in `~/.ssh`, mode 600) must hold exactly the old lines minus the
  matched ones before it is written back in place (`cat >`, so the file keeps its owner and
  mode).
- Neither install nor remove runs while a transfer to that destination is running, only one of
  them runs at a time per destination (they share one keyring entry and one comment, so a second
  one answers `busy` instead of undoing the first one's work), profile ids
  are checked to be `[A-Za-z0-9-]` before any script is built, and the install is not offered
  to an SFTP destination that signs in by password only (the line is installed through the key
  file or SSH agent).

## Consequences
- The user's personal key is no longer used by filebridge and can be removed from the server. A
  stolen laptop exposes a key that can be revoked from anywhere by deleting one line.
- Least privilege is partial: the key still runs commands as the same user (often root for
  direct mode). B or C can come later on top of this; the install and remove flows stay the same.
- Nothing is installed or changed on the server beyond one line in `authorized_keys`.
- A server that reads keys from somewhere else (`AuthorizedKeysFile`, central key management)
  fails the sign-in check, and the install is undone; Remove access reports when the old key still
  signs in, and keeps the destination on it.
- Deleting a destination that has the key forgets it locally but can't reach the server: the
  delete dialog says so and offers Remove access first.
- Rejected for now: B (creates users and sudoers rules on the user's Proxmox host or container,
  more ways to break), C (fragile: every change to filebridge's commands needs the gate updated on
  each server).
