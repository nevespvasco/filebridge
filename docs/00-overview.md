# 00 — Overview

## Goal

A desktop app for Linux, Windows and macOS, and an Android app (iOS planned; Phase 4:
[11 — Mobile](11-mobile.md)) that turns a manual migration into a 5-step wizard:
choose files or folders, choose a destination, review problems, send, and update the destination
tool (today: Nextcloud).

- The **core is generic**: it sends files anywhere a `Transport` can reach.
- **Nextcloud is the first of several integration plugins** (`Integration`); Immich, as an
  external library, is the second ([ADR 0020](adr/0020-immich-external-library.md)). Future
  candidates: Jellyfin, Paperless, Seafile, Syncthing.

## The 5-step wizard

| Step | User sees | Backed by |
|---|---|---|
| 1. Source | Drag & drop or file picker; running total of files and GB while counting | `scanner` |
| 2. Destination | Saved profile (e.g. "Nextcloud home — demo/Backup") or create one; free space and ETA | `store`, `Transport::preflight`, `Integration::preflight` |
| 3. Analysis | Name problems grouped by rule ("312 names with decomposed accents — fix automatically"); only items that need a decision stay pending | `names`, manifest |
| 4. Review & send | Final summary, "verify by checksum at the end" option, Send button | `core` pipeline |
| 5. After transfer | Integration running ("Indexing in Nextcloud: 3 folders") and final report | `Integration::after_transfer` |

Details in [06 — Interface](06-interface.md).

## Problem → feature

Each problem from the 31-hour manual Nextcloud migration maps to a feature. This table is the
acceptance checklist for v1.

| Problem in the manual migration | Feature in the app | Where it's specified |
|---|---|---|
| SSH dropped or the terminal closed mid-31 h | Jobs managed by a background service, with automatic resume | [07](07-security-reliability.md#reliability) |
| `tmux` not installed; long, fragile commands | No terminal; everything through the UI | [06](06-interface.md) |
| Password asked on every reconnect | Credentials stored in the OS keyring | [07](07-security-reliability.md#security), [ADR 0004](adr/0004-os-keyring-secrets.md) |
| Names with decomposed accents (NFD) rejected by Nextcloud | Pre-send analysis and automatic NFC correction | [04](04-name-correction.md) |
| Broken names (`PatolÃ³gica`) | Mojibake detection with a suggested correct name | [04](04-name-correction.md) |
| LXC owner and permissions (`100033`, `750/640`) | Destination profile that already knows the owner and permissions | [05](05-integration-nextcloud.md) |
| `occ files:scan` slow with no progress | Scan only uploaded folders, with visible status | [05](05-integration-nextcloud.md#after-transfer-scan) |
| Renaming after verifying confused the verification | Manifest with hash and name mapping, independent of the final name | [02](02-architecture.md), [07](07-security-reliability.md) |
| Root SSH key was left behind | Restricted key, with guided removal at the end | [07](07-security-reliability.md#security) |

## Non-goals (v1)

- No "move" mode — the source is never deleted by the app ([ADR 0005](adr/0005-fix-names-in-transit.md)).
- No dynamic plugin loading ([ADR 0006](adr/0006-compiled-in-plugins.md)).
- Scheduled sends and continuous sync are an [open decision](10-risks-and-open-decisions.md#open-decisions).
