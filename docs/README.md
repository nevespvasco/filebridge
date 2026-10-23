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
10. [09 — Roadmap](09-roadmap.md): phases 0–4 and the gate that closes each one.
11. [10 — Risks & open decisions](10-risks-and-open-decisions.md).
12. [11 — Mobile](11-mobile.md): Android (built), then iOS, with the phone as a Source.
13. [Design](design/README.md): review of reference designs, the design system, navigation and
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
| [0011](adr/0011-reclaimable-source-is-shown-never-freed.md) | *Proposed:* a reclaimable source is shown and opened in the file manager, never freed by the app |
| [0012](adr/0012-own-ssh-key-with-restrict.md) | filebridge's own SSH key in the keyring, installed with `restrict`; "Remove access" deletes it and proves it no longer signs in |
| [0013](adr/0013-phones-as-a-source-android-first.md) | On phones filebridge sends the phone's own files; Android first, iOS after, personal builds |
| [0014](adr/0014-engine-in-the-app-on-phones.md) | On phones the engine runs in the app (Android foreground service) and rclone is linked in as librclone |
| [0015](adr/0015-trust-unknown-host-key-in-the-app.md) | An unknown SSH host key is trusted in the wizard after the user checks its fingerprint; a changed key never is |
| [0016](adr/0016-nextcloud-bulk-upload.md) | Small files to Nextcloud go by its bulk upload (`transport-nextcloud`, in front of rclone); the rest through rclone |
| [0017](adr/0017-full-destination-pauses-the-job.md) | A full destination (`ErrorKind::Full`) pauses the job, resumable; free space is watched during a send |

ADR format: Context / Decision / Consequences. To change a decision, add a new ADR that supersedes
the old one rather than rewriting history.
