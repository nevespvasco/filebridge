# ADR 0019 — The app keeps the name filebridge

- Status: Accepted
- Date: 2026-10-24
- Closes the open decision "Final app name" in
  [10 — Risks and open decisions](../10-risks-and-open-decisions.md#open-decisions)

## Context
`filebridge` was a working name: the repository, the crates (`filebridge-*`), the binaries
(`filebridge`, `filebridge-service`), the bundle identifier `app.filebridge.desktop`, the
systemd unit and the keyring service all carry it. Renaming gets more expensive with every
build that stores data under it (data folder, keyring entries, Android package), and signed
installers and automatic updates (Phase 3) would fix the identifier for good.

## Decision
The final name is **filebridge**, written in lower case. Nothing is renamed.

## Consequences
- The identifiers stay as they are: `app.filebridge.desktop`, `filebridge-service.service`,
  the crate and binary names, and the data folder and keyring names derived from them.
- Signed installers, the updater and store listings (Phase 3) can be set up on these
  identifiers without a migration.
- Docs no longer call the name provisional.
