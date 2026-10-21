# filebridge

> Working name — the final name is still an [open decision](docs/10-risks-and-open-decisions.md#open-decisions).

A cross-platform desktop app (Linux, Windows, macOS) that turns a manual, terminal-driven file
migration into a 5-step wizard:

1. **Source** — pick files or folders.
2. **Destination** — pick a saved destination profile.
3. **Analysis** — review name problems (NFC, mojibake, forbidden characters…) before a single byte moves.
4. **Review & send** — the transfer runs in a background service, resumes automatically, and blocks system sleep.
5. **After transfer** — verify against a hashed manifest and update the target tool (e.g. a Nextcloud scan of only the changed folders).

The core is generic ("send files anywhere"). Nextcloud is the first of several integration plugins.

## Why

Every problem hit during a 31-hour manual migration to Nextcloud became a feature:

| Problem in the manual migration | Feature in the app |
|---|---|
| SSH dropped or the terminal closed mid-transfer | Jobs run in a background service with automatic resume |
| `tmux` not installed; long, fragile commands | No terminal — everything happens in the UI |
| Password asked on every reconnect | Credentials stored in the OS keyring |
| Names with decomposed accents (NFD) rejected by Nextcloud | Pre-send analysis and automatic NFC correction |
| Broken names (`PatolÃ³gica`) | Mojibake detection with a suggested correct name |
| LXC owner and permissions (`100033`, `750/640`) | Destination profile that already knows owner and permissions |
| `occ files:scan` slow with no progress | Scan only the uploaded folders, with visible status |
| Renaming after verifying confused the verification | Manifest with hashes and name mapping, independent of the final name |
| Root SSH key left behind | Restricted key with guided removal at the end |

## Stack

Tauri 2 + Vue 3 + TypeScript (UI), Rust + tokio (core), rclone as an embedded sidecar driven through
its `rc` API (transfer engine), SQLite (state), OS keyring via the `keyring` crate (secrets).
The core never calls rclone directly — it talks to a `Transport` contract. See [docs/01-stack.md](docs/01-stack.md).

## Status

**Phase 0 — Prototype.** Docs only; no code yet. Next: validate that per-file renaming during an
rclone transfer is fast enough (see [roadmap](docs/09-roadmap.md)).

## Documentation

Start at [docs/README.md](docs/README.md).

| Doc | Topic |
|---|---|
| [00 Overview](docs/00-overview.md) | Goal, wizard, problem → feature |
| [01 Stack](docs/01-stack.md) | Technology choices and rejected alternatives |
| [02 Architecture](docs/02-architecture.md) | Jobs, 6-stage pipeline, layers, state machine |
| [03 Extension contracts](docs/03-extension-contracts.md) | `Transport`, `NameRule`, `Integration`, `Registry` |
| [04 Name correction](docs/04-name-correction.md) | Rules, algorithm, "Repair destination" |
| [05 Nextcloud integration](docs/05-integration-nextcloud.md) | WebDAV vs direct mode, detection wizard |
| [06 Interface](docs/06-interface.md) | Wizard, screens, UX principles |
| [07 Security & reliability](docs/07-security-reliability.md) | Resume, sleep inhibit, keys, keyring |
| [08 Repository structure](docs/08-repository-structure.md) | Monorepo layout and dependency rules |
| [09 Roadmap](docs/09-roadmap.md) | Phases 0–3 with gates |
| [10 Risks & open decisions](docs/10-risks-and-open-decisions.md) | Risks, mitigations, open questions |
| [ADRs](docs/adr/) | Architecture decision records |

## Repository layout

```
filebridge/
├── apps/
│   ├── desktop/              # Tauri 2 + Vue 3 + TS
│   └── cli/                  # same core, no UI (servers, scripts)
├── crates/
│   ├── core/                 # Job, pipeline, states, Registry, progress events
│   ├── scanner/              # walk source, names as bytes, manifest
│   ├── names/                # NameRule trait + general rules
│   ├── store/                # SQLite: jobs, manifests, profiles
│   ├── secrets/              # OS keyring
│   ├── transport-api/        # Transport trait + types
│   ├── transport-rclone/     # implementation over rclone rc API
│   ├── integration-api/      # Integration trait + types
│   └── integration-nextcloud/# WebDAV, direct mode, occ, own rules
├── sidecars/                 # rclone binaries per platform (not committed)
└── docs/
```
