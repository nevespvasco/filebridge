# ADR 0015 — Trust an unknown SSH host key in the app, after the user checks its fingerprint

- Status: Accepted
- Date: 2026-10-22
- Changes: the "no accept-unknown path" rule of `crates/integration-nextcloud` (`ssh.rs`, README)
  and [07 — Security](../07-security-reliability.md#security)
- Numbered 0015 because 0013 and 0014 (phones, ADRs written alongside) were taken first.

## Context
filebridge checks every SSH server's host key against a `known_hosts` file (default
`$HOME/.ssh/known_hosts`), both in russh (Nextcloud direct mode, the SSH-key operations) and in
rclone (`known_hosts_file`, SFTP transfers). An unknown host was always an error that said
"connect once with `ssh`": the user had to open a terminal, run `ssh`, compare the fingerprint
and type `yes` before the wizard would work.

That is a terminal step in an app built to need none, and on a phone it is impossible: the
engine runs inside the app, `HOME` is the app's own folder, and there is no `ssh` to run
(ADR 0014, docs/11). SSH profiles could not be created there at all.

Options weighed:
- (A) keep the rule, document a manual edit of `known_hosts`: impossible on a phone;
- (B) accept unknown keys automatically on first use (TOFU without asking): an attacker on the
  network during setup would be trusted silently;
- (C) show the key and let the user trust it, as `ssh` does, but inside the wizard.

## Decision
**C.** Two ipc operations:
- `scan_host_key(host, port, known_hosts_file)` connects only far enough to receive the host
  key (it never signs in) and answers `known`, `unknown` or `changed`, with the key type, its
  `SHA256:` fingerprint, the file, and for `changed` the line on record. It writes nothing.
- `trust_host_key(host, port, known_hosts_file, fingerprint)` scans again and appends the key
  only if the host has **no key at all** on record and the server still presents the key whose
  fingerprint the user confirmed. Anything else is refused with `host_key_changed`.

Rules:
- **A changed key is never accepted from the app.** If the file holds any key for the host
  (of any type) that isn't the one presented, the answer is `changed`, the wizard shows it as
  an error with no "trust" button, and the user must remove the old line themselves after
  checking with whoever runs the server.
- **Only the recorded key types are offered.** Connections offer the server only the key types
  `known_hosts` holds for that host (as `ssh` does, and as rclone already did), so a server with
  an Ed25519 and an ECDSA key presents the one on record, and an attacker can't dodge the check
  by presenting a type that isn't recorded.
- **Append only.** The file is never rewritten: a new line `host keytype base64` (or
  `[host]:port …` off port 22) goes at the end, after ending a last line that lacks its newline.
  Hashed (`|1|…`), commented and other hosts' lines stay byte for byte. The line is written in
  plain form, not hashed, so the user can see and remove it.
- **New files are private.** A missing `~/.ssh` is created 0700 and `known_hosts` 0600.
- **Only plain names and addresses** (`[A-Za-z0-9._:-]`) are written: a host with `,`, spaces,
  wildcards or a leading `|`/`@`/`!` would make the line match other hosts. A custom
  `known_hosts_file` must be a full path (no `~` expansion that could create a literal `~`
  folder).
- Host certificates stay unsupported (an error, as before).
- One trust at a time per service process; the append re-reads the file right before writing.

The wizards (SFTP in `ProfileForm.vue`, Nextcloud direct mode in `NextcloudDetect.vue`) scan
before they first connect (Save, Browse, Detect). A known host goes straight through; an
unknown one shows "Trust this server?" with the server, key type, fingerprint, how to check it
on the server (`ssh-keygen -lf /etc/ssh/ssh_host_<type>_key.pub`) and the file it will be added
to, with Cancel focused. Saving an SFTP destination therefore needs the server to be reachable.

## Consequences
- SSH profiles can be set up without a terminal, on the desktop and on phones.
- The security of the first connection rests on the user comparing the fingerprint, exactly as
  with `ssh`. The dialog says why and how; it can't force the comparison.
- `known_hosts` is shared with the user's own `ssh`: a host trusted in filebridge is trusted by
  `ssh` too, and vice versa. That is what users expect from "the usual file".
- A host whose key legitimately changed still needs a manual edit (or `ssh-keygen -R`); a
  guided "replace the key" flow would need its own decision.
- Existing profiles whose host is unknown (e.g. `known_hosts` was cleared) still fail with an
  error until the profile is re-created or the host is trusted in a wizard; a "trust" action
  on Destinations is a possible follow-up.
