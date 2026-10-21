# 08 — Repository structure

A monorepo with a Cargo workspace and a Vue project. Each crate maps to one feature, and the CLI
uses the same core as the app, so it also serves headless servers.

```
filebridge/
├── apps/
│   ├── desktop/                 # Tauri 2 + Vue 3 + TS
│   │   ├── src/                 # interface (views, Pinia stores, components)
│   │   └── src-tauri/           # Tauri commands — only bridge to core
│   └── cli/                     # same logic, no UI (servers, scripts)
├── crates/
│   ├── core/                    # Job, pipeline, states, Registry, progress events
│   ├── scanner/                 # walk source, names as bytes, manifest
│   ├── names/                   # NameRule trait + general rules (NFC, mojibake, forbidden)
│   ├── store/                   # SQLite: jobs, manifests, profiles
│   ├── secrets/                 # OS keyring
│   ├── transport-api/           # Transport trait + types
│   ├── transport-rclone/        # implementation over rclone's rc API
│   ├── integration-api/         # Integration trait + types
│   └── integration-nextcloud/   # WebDAV, direct mode, occ, own rules
├── sidecars/                    # rclone binaries per platform
└── docs/
```

Every folder has its own `README.md` with responsibility, planned API, allowed dependencies and
the phase it lands in.

## Dependency rules

```mermaid
flowchart TB
    desktop["apps/desktop (src-tauri)"] --> core
    cli["apps/cli"] --> core
    desktop --> trc["transport-rclone"]
    desktop --> inc["integration-nextcloud"]
    cli --> trc
    cli --> inc
    core --> tapi["transport-api"]
    core --> iapi["integration-api"]
    core --> names
    core --> scanner
    core --> store
    core --> secrets
    trc --> tapi
    inc --> iapi
    inc --> tapi
    inc --> names
    iapi --> tapi
    iapi --> names
    scanner --> names
```

- `core` depends only on `*-api` crates (plus `names`, `scanner`, `store`, `secrets`), **never on
  implementations**.
- Every `transport-*` and `integration-*` crate is independent: **a new plugin is a new crate plus
  one line in the `Registry`**. Plugins never depend on `core`, `store` or `secrets`.
- Only the app binaries depend on concrete plugins (to register them).
- The interface talks only through typed Tauri commands and events (generated with `specta`), so
  the TS types come from Rust and never drift.

Enforce in CI (Phase 1): a `cargo deny`/`cargo-depgraph` check or a small script that fails if
`crates/core/Cargo.toml` lists a `transport-*`/`integration-*` implementation crate.

## Workspace conventions (to scaffold in Phase 0)

- Root `Cargo.toml` with `[workspace]`, `resolver = "2"`, shared `[workspace.dependencies]`
  versions, `edition = "2021"` (or 2024 if the toolchain allows).
- `pnpm` for the Vue project (`apps/desktop`).
- `rust-toolchain.toml` pinned to stable.
- Package names prefixed `filebridge-` (`filebridge-core`, …) to avoid collisions; folder names
  stay short.

The name `filebridge` is provisional.
