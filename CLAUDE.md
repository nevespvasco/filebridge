# filebridge — agent guide

Cross-platform file-transfer app: Tauri 2 + Vue 3/TS UI, Rust core, rclone sidecar (librclone
in the process on phones).
Design docs live in `docs/` — read `docs/README.md` first, and the ADRs in `docs/adr/` before
changing anything they cover.

## Current phase

Phase 1 (personal MVP). See `docs/09-roadmap.md` for deliverables and the gate that closes each
phase. The Phase 0 gate passed provisionally on simulated results (ADR 0007, plan A); the
real-server benchmark is still pending and can reopen it.

## Build

- Rust toolchain: `~/.cargo/bin/cargo` (not on PATH by default). rclone: `~/.local/bin/rclone`.
- Android (docs/11): Go `~/.local/go/bin/go`, SDK `~/Android/Sdk` (NDK under `ndk/`). On macOS:
  Go and JDK 17 from Homebrew, SDK `~/Library/Android/sdk`.
  `sidecars/librclone/build.sh android`, then `pnpm tauri android build` in `apps/desktop` with
  `ANDROID_HOME`, `NDK_HOME` and `JAVA_HOME` (JDK 17 or 21) set.
- `cargo test --workspace` and `cargo clippy --workspace -- -D warnings` must pass.
- Shared external dependency versions live in the root `[workspace.dependencies]`; add a new one
  there only if more than one crate needs it.

## Hard rules

- **Layering:** Vue UI → Tauri commands → `ipc` → `filebridge-service` → `core` → contracts
  (`transport-api`, `integration-api`, `names`) → plugins. The UI reaches `core` only through
  `ipc` (ADR 0008); `core` talks only to contracts. On phones (ADR 0014) there is no service:
  the app's commands call `host::EngineOperations`, the same `ipc::Operations` contract.
- **`ipc` never depends on `core`**: the types both sides share live in `model`.
- **`core` depends only on `*-api` crates** (and `model`, `names`, `store`, `scanner`,
  `secrets`), never on a concrete `transport-*` or `integration-*` crate. Only `crates/host` (used by `apps/service`,
  `apps/cli` and the app's phone build) wires implementations into the `Registry`.
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
