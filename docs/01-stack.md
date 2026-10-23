# 01 — Stack

Tauri 2 + Vue 3 + TypeScript for the interface, Rust in the core, and rclone as the transfer
engine. It reuses what we already know well (Vue/TS) and avoids rewriting the hardest problem:
moving bytes reliably.

## Choices

| Layer | Choice | Why | Rejected alternative |
|---|---|---|---|
| Interface | Vue 3 + TypeScript + Vite (Pinia for state) | Primary stack of the author | React (works too, no gain) |
| Desktop shell | Tauri 2 | Small installers on all 3 OSes, Rust core, native sidecars | Electron (heavy, Node core) |
| Core | Rust (tokio) | Walks hundreds of thousands of files fast; strong types for plugin contracts | Go (good, but doesn't integrate with Tauri) |
| Transfer engine | rclone, embedded as a sidecar, driven via its `rc` API | Works on Windows (rsync doesn't); supports SFTP, SMB, WebDAV, S3 and 40+ backends; has resume, checksums and progress | Reimplementing rsync/SFTP (months of work and bugs) |
| State | SQLite | Jobs, manifests and history survive restarts | JSON files |
| Credentials | OS keyring (`keyring` crate) | Keychain on macOS, Credential Manager on Windows, Secret Service on Linux | Plain-text config file |

## Supporting crates (planned)

| Purpose | Crate | Notes |
|---|---|---|
| Async runtime | `tokio` | Multi-threaded runtime in core and transports |
| Async traits | `async-trait` | Used by `Transport` and `Integration` (object-safe `dyn` traits) |
| SQLite | `rusqlite` (bundled) or `sqlx` | Prefer `rusqlite` + a dedicated writer task; migrations embedded |
| Unicode normalization | `unicode-normalization` | NFC/NFD checks and conversion |
| Encoding detection | `encoding_rs` (+ `chardetng`) | Decode Latin-1/CP1252 names |
| Hashing | `sha2` | SHA-256 for the manifest ([ADR 0017](adr/0017-sha256-for-the-manifest.md)) |
| Keyring | `keyring` | Per-OS backends |
| SSH (direct mode setup) | `russh` or `openssh` | Detection wizard and `occ` calls |
| HTTP (rclone rc, WebDAV checks) | `reqwest` | rclone `rc` is a local HTTP JSON API |
| TS type generation | `specta` + `tauri-specta` | Tauri commands and events typed end to end |
| Errors | `thiserror` (libs), `anyhow` (bins) | |
| Logging | `tracing` | Never log credentials |
| CLI | `clap` | `apps/cli` |

Versions are pinned when the Cargo workspace is scaffolded in Phase 0.

## rclone also speaks WebDAV

rclone talks to Nextcloud directly over WebDAV. That gives a second send mode that needs neither
SSH nor root (see [05 — Nextcloud](05-integration-nextcloud.md)).

## Key decision: the core never calls rclone directly

The core talks to a `Transport` contract; rclone is only the first implementation. If rclone ever
falls short for a destination, the implementation is swapped without touching anything else.
See [03 — Extension contracts](03-extension-contracts.md) and [ADR 0002](adr/0002-rclone-behind-transport.md).
