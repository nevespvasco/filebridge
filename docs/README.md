# filebridge docs

These docs are the implementation plan. They expand the original plan (2026-10-21) into enough
detail to build each crate on its own.

## Reading order

1. [00 — Overview](00-overview.md): what the app is and why each feature exists.
2. [01 — Stack](01-stack.md): technologies and the alternatives we rejected.
3. [02 — Architecture](02-architecture.md): jobs, the 6-stage pipeline, layers, state machine.
4. [03 — Extension contracts](03-extension-contracts.md): `Transport`, `NameRule`, `Integration`, `Registry`.
5. [04 — Name correction](04-name-correction.md): rules, the algorithm, "Repair destination".
6. [05 — Nextcloud integration](05-integration-nextcloud.md): WebDAV mode, direct mode, detection wizard.
7. [06 — Interface](06-interface.md): the wizard, screens and UX principles.
8. [07 — Security & reliability](07-security-reliability.md): resume, sleep inhibit, keys, keyring.
9. [08 — Repository structure](08-repository-structure.md): monorepo layout and dependency rules.
10. [09 — Roadmap](09-roadmap.md): phases 0–5 and the gate that closes each one.
11. [10 — Risks & open decisions](10-risks-and-open-decisions.md).
12. [11 — Mobile](11-mobile.md): Android (built), then iOS, with the phone as a Source.
13. [Runbook: real-server tests](runbooks/real-server-tests.md): how to run each gate against
    the real servers and a phone, and what to send back.
14. [Design](design/README.md): review of reference designs, the design system, navigation and
    feature specs D01–D11 (all *Proposed*).

## Architecture decision records

| ADR | Decision |
|---|---|
| [0001](adr/0001-tauri-vue-rust.md) | Tauri 2 + Vue 3 + Rust core instead of Electron |
| [0002](adr/0002-rclone-behind-transport.md) | rclone as a sidecar, driven via `rc`, hidden behind `Transport` |
| [0003](adr/0003-sqlite-state.md) | SQLite for jobs, manifests and profiles |
| [0004](adr/0004-os-keyring-secrets.md) | Secrets only in the OS keyring |
| [0005](adr/0005-fix-names-in-transit.md) | Fix names in transit, never at the source |
| [0006](adr/0006-compiled-in-plugins.md) | Plugins compiled into the app in v1; WASM later |
| [0007](adr/0007-rename-in-transit-strategy.md) | *Proposed:* rename in transit with one `copyfile` per file (plan A) |
| [0008](adr/0008-service-ipc.md) | Engine in `filebridge-service`; window ↔ service IPC over a local socket |
| [0009](adr/0009-nextcloud-on-the-ssh-host.md) | Nextcloud direct mode also on the SSH host itself (no `pct`), not only through the Proxmox node |
| [0010](adr/0010-bandwidth-limit-and-send-window.md) | One app-wide bandwidth limit and a daily send window (transfer waits outside it) |
| [0011](adr/0011-reclaimable-source-is-shown-never-freed.md) | A reclaimable source is shown and opened in the file manager, never freed by the app |
| [0012](adr/0012-own-ssh-key-with-restrict.md) | filebridge's own SSH key in the keyring, installed with `restrict`; "Remove access" deletes it and proves it no longer signs in |
| [0013](adr/0013-phones-as-a-source-android-first.md) | On phones filebridge sends the phone's own files; Android first, iOS after, personal builds |
| [0014](adr/0014-engine-in-the-app-on-phones.md) | On phones the engine runs in the app (Android foreground service) and rclone is linked in as librclone |
| [0015](adr/0015-trust-unknown-host-key-in-the-app.md) | An unknown SSH host key is trusted in the wizard after the user checks its fingerprint; a changed key never is |
| [0016](adr/0016-nextcloud-bulk-upload.md) | Small files to Nextcloud go by its bulk upload (`transport-nextcloud`, in front of rclone); the rest through rclone |
| [0017](adr/0017-sha256-for-the-manifest.md) | The manifest uses SHA-256, on the source and at the destination; no xxHash |
| [0018](adr/0018-scope-personal-use-scheduled-and-copy-only-sync.md) | Personal use, the name stays `filebridge`; scheduled sends in Phase 3, copy-only continuous sync (never deletes at the destination) in Phase 5 |
| [0019](adr/0019-scheduled-sends.md) | Scheduled sends: a run is an ordinary job that sends only files new or changed (size, mtime) since the schedule last sent them; a changed file keeps its older version at the destination under a dated name; missed times run once; in the service only |
| [0020](adr/0020-immich-external-library.md) | Immich as an external library: files over SFTP (or into a local folder), then a library scan through its API; an `Integration`, not a `Transport` |
| [0021](adr/0021-full-destination-pauses-the-job.md) | A full destination (`ErrorKind::Full`) pauses the job, resumable; free space is watched during a send |
| [0022](adr/0022-smb-and-s3-as-rclone-backends.md) | SMB and S3 are `Backend` variants of `transport-rclone`; `Backend` answers its kind and password slot, so `core` never lists the variants |
| [0023](adr/0023-continuous-sync.md) | Continuous sync is a watched schedule (`on_change`): OS file events (`notify`) start a run a quiet time (1 min) after changes stop, files still being written wait, an hourly rescan (10 min where the folder can't be watched) and one at each start catch what events miss; waits for the send window; desktop only; never deletes at the destination |
| [0024](adr/0024-nextcloud-mode-advice-and-occ-user.md) | The Nextcloud wizard probes the server and recommends direct mode or WebDAV, with the reason; an opt-in dedicated user in the web server's group, `occ` through a sudoers rule the user adds by hand |
| [0025](adr/0025-report-diff-and-sealed-export.md) | Two reports are compared file by file by source path; a sealed export is a SHA-256 digest chain with a readable HTML copy, no signing key |
| [0026](adr/0026-unreadable-source-pauses-the-job.md) | A source that can't be read (a disk unplugged) pauses the job, resumable, instead of failing its files |
| [0027](adr/0027-source-thumbnails-in-the-cache.md) | Thumbnails of source images: made by the engine (`crates/thumbnail`), cached in the app's cache (128 MB, LRU), sent to the window as bytes; no video previews |
| [0028](adr/0028-login-start-on-every-desktop-os.md) | The engine starts at login from a per-user entry on every desktop OS: systemd unit, LaunchAgent, Run key value (through the app, so no console window) |
| [0029](adr/0029-integration-checkpoints-skips-and-probe-files.md) | Integration checkpoints, `will_skip` (files a tool won't take) and probe files the transport may remove; Immich waits for its scan, warns about skipped files and checks it sees the folder |
| [0030](adr/0030-discarded-drafts.md) | A draft the New-transfer wizard replaces is discarded (Cancelled, kept, hidden from the Transfers list and Activity), not shown as cancelled |
| [0031](adr/0031-one-word-for-a-transfer.md) | The UI calls a Job a Transfer, in Portuguese *Envio*, everywhere; "send" is only a verb |
| [0032](adr/0032-keep-the-name-filebridge.md) | The app keeps the name `filebridge` (the open decision, closed; ADR 0018 had said so in passing) |
| [0033](adr/0033-give-up-on-a-file-after-three-retries.md) | A file that keeps failing is retried three times, then given up on and listed at the end; a destination that doesn't answer uses up no file's retries |
| [0034](adr/0034-ios-without-a-mac.md) | iOS is built on GitHub's Mac (librclone linked in, `gen/apple` made per run) and tested in the simulator and on the author's iPhone with a free Apple ID; Sources are Files-app folders kept with bookmarks; sends run while the app is open |
| [0035](adr/0035-home-network-nextcloud.md) | A Nextcloud at home: per destination, trust its certificate (rclone's `no_check_certificate` for that remote only) or allow http:// when the host is on the home network or Tailscale |

ADR format: Context / Decision / Consequences. To change a decision, add a new ADR that supersedes
the old one rather than rewriting history.
