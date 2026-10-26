# filebridge

A file-transfer app for Linux, Windows, macOS and Android that turns a manual, terminal-driven
file migration into a 5-step wizard:

1. **Source** — pick files or folders (on a phone: the phone's own files).
2. **Destination** — pick a saved destination profile, or create one.
3. **Analysis** — review name problems (NFC, mojibake, forbidden characters…) before a single byte moves.
4. **Review & send** — the transfer runs in a background service, resumes automatically, and blocks system sleep.
5. **After transfer** — verify against a SHA-256 manifest and update the target tool (a Nextcloud
   scan of only the changed folders, an Immich library scan).

The core is generic ("send files anywhere a `Transport` reaches"); Nextcloud and Immich are its
integration plugins. Built for personal use ([ADR 0018](docs/adr/0018-scope-personal-use-scheduled-and-copy-only-sync.md)).

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

## What it does

- **Destinations:** local folder, SFTP, SMB shares, S3 buckets (AWS, MinIO and compatible),
  Nextcloud over WebDAV or in direct mode (SSH to the Nextcloud machine or its Proxmox node,
  with the right owner and a scan by path), and Immich as an external library.
- **Names fixed in transit, never at the source:** NFC, forbidden and reserved characters,
  trailing dots and spaces, length, mojibake, collisions. The source → destination mapping
  lives in the manifest. *Repair destination* fixes names already sent.
- **Never deletes or changes the source.** There is no "move" mode; after a verification with
  zero differences the app shows what could be freed and opens it in the file manager.
- **Reliability:** every job stage is stored in SQLite, so a job resumes after a crash, a
  reboot, a full destination or an unplugged source disk. App-wide bandwidth limit and a daily
  send window.
- **Verification and reports:** per-file SHA-256 on both sides; CSV, JSON or sealed (digest
  chain + readable HTML) reports, and a file-by-file diff of two reports.
- **Scheduled sends:** a saved send runs at set times and sends only what changed; a changed
  file keeps its older version at the destination under a dated name.
- **Continuous sync, copy-only:** a watched folder sends new and changed files a minute after
  changes stop. Nothing is ever deleted at the destination (desktop only).
- **Security:** secrets only in the OS keyring (Android Keystore on phones); filebridge's own
  restricted SSH key, with "Remove access" that proves it no longer signs in.
- **Interfaces:** the desktop app (pt and en), a tray icon, an engine that can start at login,
  and the `filebridge` CLI over the same engine, for servers and scripts.

## Stack

Tauri 2 + Vue 3 + TypeScript (UI), Rust + tokio (core), rclone as a sidecar driven through its
`rc` API (librclone linked into the app on phones), SQLite (state), OS keyring (secrets).
The core never calls rclone directly — it talks to a `Transport` contract, and new
destinations and tools are plugin crates registered in one place. See
[docs/01-stack.md](docs/01-stack.md) and [docs/02-architecture.md](docs/02-architecture.md).

## Getting started

Prerequisites: the Rust stable toolchain, Node with `pnpm`, rclone (the tested version is in
[`sidecars/RCLONE_VERSION`](sidecars/RCLONE_VERSION)), and on Linux the Tauri system libraries
(`libwebkit2gtk-4.1-dev libgtk-3-dev libayatana-appindicator3-dev librsvg2-dev libsoup-3.0-dev`).

```bash
cargo test --workspace
cargo clippy --workspace -- -D warnings
```

Desktop app ([apps/desktop](apps/desktop/README.md)):

```bash
cd apps/desktop
pnpm install
pnpm tauri dev                               # real service and engine
FILEBRIDGE_FAKE_TRANSPORT=1 pnpm tauri dev   # no rclone: a local-disk fake transport
VITE_MOCK=1 pnpm dev                         # UI only, in a browser, against a mock service
pnpm bundle                                  # .deb/.rpm with the service, CLI and rclone inside
```

CLI ([apps/cli](apps/cli/README.md)):

```bash
cargo install --path apps/cli --locked
filebridge profile add-sftp --name nas --host nas.example.com --root /backup
filebridge send ~/Documents --to nas --verify
```

Android: see [docs/11-mobile.md](docs/11-mobile.md) (`sidecars/librclone/build.sh android`,
then `pnpm tauri android build`).

## Documentation

Start at [docs/README.md](docs/README.md); the words used throughout are in
[GLOSSARY.md](GLOSSARY.md).

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
| [09 Roadmap](docs/09-roadmap.md) | Phases 0–5 with gates |
| [10 Risks & open decisions](docs/10-risks-and-open-decisions.md) | Risks, mitigations, open questions |
| [11 Mobile](docs/11-mobile.md) | Android (built), then iOS |
| [Design](docs/design/README.md) | Design system, navigation, feature specs D01–D11 |
| [Runbook](docs/runbooks/real-server-tests.md) | Running each gate against real servers and a phone |
| [ADRs](docs/adr/) | Architecture decision records |

## Repository layout

```
filebridge/
├── apps/
│   ├── desktop/                # Tauri 2 + Vue 3 + TS; Android project under src-tauri/gen/android
│   ├── service/                # filebridge-service: hosts the engine, serves ipc
│   └── cli/                    # filebridge: same engine, no UI
├── crates/
│   ├── core/                   # Job, pipeline, states, Registry, scheduler, progress events
│   ├── host/                   # wires the engine: data dir, rclone, built-in plugins
│   ├── ipc/                    # window/CLI ↔ service messages over a local socket
│   ├── model/                  # types the UI and the service share
│   ├── scanner/                # walk source, names as bytes, manifest
│   ├── names/                  # NameRule trait + general rules
│   ├── store/                  # SQLite: jobs, manifests, profiles, schedules
│   ├── secrets/                # OS keyring
│   ├── thumbnail/              # previews of source images, bounded cache
│   ├── transport-api/          # Transport trait + types
│   ├── transport-rclone/       # SFTP, WebDAV, SMB, S3, local over rclone's rc API
│   ├── transport-nextcloud/    # small files to Nextcloud by bulk upload
│   ├── transport-fake/         # local-disk transport with fault injection (tests, dev)
│   ├── integration-api/        # Integration trait + types
│   ├── integration-nextcloud/  # WebDAV, direct mode, occ, own rules
│   └── integration-immich/     # Immich external library, scan through its API
├── sidecars/                   # pinned rclone version, desktop sidecar scripts, librclone build
├── tools/                      # benchmark helpers
└── docs/
```

Every folder has its own `README.md`. Contributors (and agents) should also read
[CLAUDE.md](CLAUDE.md) for the layering and hard rules.
