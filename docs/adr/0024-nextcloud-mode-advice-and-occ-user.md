# ADR 0024 — The Nextcloud wizard recommends a mode from the server; a dedicated user with an `occ`-only sudoers rule

- Status: Accepted
- Date: 2026-10-23

## Context
[05](../05-integration-nextcloud.md#mode-auto-selection) planned to choose the Nextcloud mode
"automatically", but the user picked it in the wizard with nothing to go on but two sentences.
Whether direct mode can work is a fact about the server (does SSH reach the machine Nextcloud
runs on, as a user with the rights direct mode needs?), and the wizard can find it out the same
way direct mode's detection does.

A second gap: direct mode needs root on the Nextcloud machine (or passwordless `sudo -n` for
everything), because SFTP writes as the SSH user and the files then need the web server's owner.
[07](../07-security-reliability.md#security) asked for a dedicated user with a `sudoers` rule
limited to `occ`; [ADR 0012](0012-own-ssh-key-with-restrict.md) left that out because it
creates users and sudoers rules on the user's server. A sudoers rule for `chown` would have to
name paths, and sudoers wildcards match `/` and `..`, so it can't be narrowed to the data folder.

## Decision
**Mode advice.** The wizard offers "Not sure which to pick? Let filebridge check the server": the
SSH server, port and user, and the Nextcloud user. `probe_nextcloud` runs the same detectors as
`detect_nextcloud` (`Nextcloud::advise` in the plugin) and saves nothing:
- every detection step passes → **direct** (`ready`);
- SSH does not connect, or the user doesn't trust the server's key → **WebDAV** (`unreachable`,
  `untrusted`, the latter decided in the window);
- a step fails for want of rights (`pct list` or the owner fix needs root or `sudo -n`, or the SSH
  user can't write the user's files folder: `Detection::lacks_rights`) → **WebDAV** (`rights`);
- any other failure (no container, `config.php`, data folder or `occ`) → **WebDAV**
  (`not_found`).

The window switches the mode control to the recommendation, says why (pt and en, the failing
step's own message below), fills the chosen form with what was typed, and leaves the control
free: the user can always pick the other mode. The size-based suggestion from docs/05 (direct
mode above 20 GB or 50 000 files) is not part of this.

**A dedicated user, opt-in** ("Dedicated user instead of root" in the direct-mode form,
`--dedicated-user` in the CLI; `Access::Dedicated` in the profile). Only on the machine Nextcloud
runs on (`Runtime::Host`; `pct` needs root, so the Proxmox node is skipped). The user:
- is **in the web server's group** and never root: it owns what it sends and gives it the web
  server's group with `chgrp` (allowed to any owner, for a group it is in), with group-writable
  modes `770`/`660`, so Nextcloud reads and writes the files through the group. No `chown`, no
  `sudo` for any host command;
- can **write the folder sends go into**, made group-writable once (`chmod -R g+rwX`);
- runs `occ` through **one sudoers line** naming only the commands direct mode uses, as the web
  user, with `php` by its full path:
  `filebridge ALL=(www-data) NOPASSWD: /usr/bin/php /var/www/nextcloud/occ status --output\=json, /usr/bin/php /var/www/nextcloud/occ user\:info alice --output\=json, /usr/bin/php /var/www/nextcloud/occ files\:scan *`
  (`RunAs::SudoRule`). Long `occ` commands keep dying with the SSH connection: the watcher runs
  outside `sudo` as the SSH user and signals `sudo`, which relays SIGTERM to `occ`.

filebridge **never sets this up itself**: detection (as that user) reads what it can and
returns the server setup (`useradd`/`usermod`, the `chmod`, the sudoers line written to
`/etc/sudoers.d/filebridge` with `chmod 0440` and `visudo -cf`) built from what it found, for
the user to read and run as root; values it couldn't read are placeholders, never guesses. Each
missing right fails its step (`group`, `datadirectory`, `writable`, `run-as`) saying which step
of the setup to run. A rule is not written when a name holds characters a rule can't hold.

## Consequences
- Detection becomes a read-only probe as well as the wizard; any detector added later (Docker,
  Snap) is covered by the advice without changes to it.
- One more SSH sign-in when the user asks for advice, then a second one when they detect and
  save in direct mode.
- `probe_nextcloud`, `dedicated_user` and `server_setup` are protocol 25.
- A dedicated user's files belong to it, not to the web user: Nextcloud reaches them through the
  group. Nextcloud doesn't check owners, but anything on the server that expects `www-data` as
  the owner (a backup script, a later `chown -R` by hand) sees `filebridge:www-data`.
- The permission fix picks the folders a send created by their owner, the SSH user (docs/05).
  A dedicated user keeps owning its folders, so every folder it ever made gets the mode and
  group again on each send that passes through it. They are its own folders; Nextcloud's are
  left alone.
- Partly supersedes [ADR 0012](0012-own-ssh-key-with-restrict.md)'s "Rejected for now: B": B is
  offered as an option the user sets up by hand, after reading what filebridge proposes.
