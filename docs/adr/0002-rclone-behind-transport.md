# ADR 0002 — rclone as sidecar, behind the `Transport` contract

- Status: Accepted
- Date: 2026-10-21

## Context
Reliable transfer (resume, checksums, progress, many backends, Windows support) is the hardest
part of the problem. rsync doesn't run natively on Windows; reimplementing SFTP/rsync would take
months and bring bugs.

## Decision
Embed **rclone** as a Tauri sidecar, run it as `rclone rcd` on localhost with a per-start random
credential, and drive it through its **`rc` HTTP API**. `core` never calls rclone directly: it
talks to the **`Transport` trait**, and `crates/transport-rclone` is the first implementation.

## Consequences
- SFTP, SMB, WebDAV (incl. Nextcloud), S3 and 40+ backends available from day one.
- rclone can be replaced per destination without touching `core`.
- We ship a third-party binary per platform (MIT licence — include it).
- Open risk: per-file renaming speed — validated in Phase 0 (see [roadmap](../09-roadmap.md)).
