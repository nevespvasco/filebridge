# crates/integration-nextcloud

First `Integration`: **Nextcloud**. **Lands in:** Phase 1 (direct mode), Phase 2 (WebDAV mode).

## Responsibility
- Two modes: **WebDAV** (default; app password; no scan needed) and **direct** (SFTP into the data
  folder; owner/permissions from the profile; `occ files:scan --path` on changed folders).
- Profile detection wizard: WebDAV test → `pct list`/`pct config` → `datadirectory` from
  `config.php` → `occ` path and sudo vs `su -s /bin/bash www-data` → idmap UID (`100000 + 33`) →
  save profile. Structured as pluggable `Detector`s (LXC first; Docker/AIO/Snap later).
- Name rules: `nextcloud-nfc`, `nextcloud-reserved` (`.htaccess`, `.part`, …).
- Preflight: auth, quota, data path writable, `occ status`, free disk.
- After transfer: minimal folder set, scan with parsed progress, `incompatible encoding` →
  visible error with "Repair destination".

## Allowed dependencies
`integration-api`, `transport-api`, `names`, `reqwest` (WebDAV), an SSH client crate
(`russh`/`openssh`).

See [docs/05-integration-nextcloud.md](../../docs/05-integration-nextcloud.md).
