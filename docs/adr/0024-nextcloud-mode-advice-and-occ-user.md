# ADR 0024 — The Nextcloud wizard recommends a mode from the server; a dedicated user with an `occ`-only sudoers rule

- Status: Accepted
- Date: 2026-10-23

## Context
[05](../05-integration-nextcloud.md#mode-auto-selection) planned to choose the Nextcloud mode
"automatically", but the user picked it in the wizard with nothing to go on but two sentences.
Whether direct mode can work is a fact about the server (does SSH reach the machine Nextcloud
runs on, as a user with the rights direct mode needs?), and the wizard can find it out the same
way direct mode's detection does.

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

## Consequences
- Detection becomes a read-only probe as well as the wizard; any detector added later (Docker,
  Snap) is covered by the advice without changes to it.
- One more SSH sign-in when the user asks for advice, then a second one when they detect and
  save in direct mode.
- `probe_nextcloud` is protocol 25.
