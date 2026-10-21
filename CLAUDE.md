# filebridge — agent guide

Cross-platform file-transfer app: Tauri 2 + Vue 3/TS UI, Rust core, rclone sidecar.
Design docs live in `docs/` — read `docs/README.md` first, and the ADRs in `docs/adr/` before
changing anything they cover.

## Current phase

Phase 0 (prototype). See `docs/09-roadmap.md` for deliverables and the gate that closes each phase.
Do not build Phase 1+ features before the Phase 0 gate is decided.

## Hard rules

- **Layering:** Vue UI → Tauri commands → `core` → contracts (`transport-api`, `integration-api`,
  `names`) → plugins. The UI talks only to `core`; `core` talks only to contracts.
- **`core` depends only on `*-api` crates** (and `names`, `store`, `scanner`, `secrets`), never on a
  concrete `transport-*` or `integration-*` crate. Only the app binaries wire implementations into
  the `Registry`.
- **A new plugin is a new crate plus one line in the `Registry`.** If adding one needs a change in
  `core`, the contract is wrong — fix the contract.
- **UI ↔ Rust types are generated with `specta`.** Never hand-write a TS type that mirrors a Rust one.
- **Never delete or modify the source.** No "move" mode. Freeing the source is a manual action,
  offered only after a verification with zero differences.
- **Names are fixed in transit, never at the source.** The destination receives the corrected name;
  the source→destination mapping lives in the manifest.
- **Secrets only in the OS keyring.** Never in config files, SQLite, logs, or error messages.
- **Every job stage is persisted to SQLite** before moving on, so a job can stop and resume at any stage.
- **Destructive actions require confirmation** and show what will happen.

## Conventions

- Docs and code comments in English. UI strings in pt and en.
- When a decision changes, update the relevant doc and add/supersede an ADR in the same commit.
