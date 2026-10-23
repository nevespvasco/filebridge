# ADR 0018 — Scope: personal use, the name stays, scheduled sends and copy-only sync

- Status: Accepted
- Date: 2026-10-23
- Closes three open decisions in [10](../10-risks-and-open-decisions.md#open-decisions):
  personal use or distribution, scheduled sends and continuous sync, and the final name

## Context
Three questions in [10](../10-risks-and-open-decisions.md) set the order of the later phases.
They were left open until the Phase 1 MVP worked:
- Distribution decides how early code signing, installers and user docs matter.
- Recurring sends and continuous sync decide what Phase 3 holds and whether a new phase is
  needed. One-off sends with a daily window are already built
  ([ADR 0010](0010-bandwidth-limit-and-send-window.md)).
- The name is in the Android package id, the keyring service, the systemd unit and the data
  folders. Changing it later moves all of them.

## Decision
- **Personal use, for now.** Builds are for the author's own machines and phones. Signed
  installers and automatic updates stay in Phase 3, deferred. The Android APK is signed with the
  author's own key and not published to a store.
- **The name stays `filebridge`.** The Android package id becomes `app.filebridge`.
- **Scheduled sends enter Phase 3.** A saved send runs at set times and sends only what changed
  since its last run. It runs in the service, never in the window, and waits for the daily send
  window like any send. Its own ADR settles the details before the code.
- **Continuous sync enters as Phase 5, copy-only.** The service watches a source folder and sends
  what changes, built on the scheduled sends. Its own ADR comes before the code. Two rules are
  fixed here:
  - **Nothing is ever deleted at the destination.** A file deleted at the source stays at the
    destination.
  - **A rename at the source is a new file.** The new name is sent, and the old one stays at the
    destination.
- The source is never touched, as everywhere else ([ADR 0005](0005-fix-names-in-transit.md),
  [ADR 0011](0011-reclaimable-source-is-shown-never-freed.md)).

## Consequences
- The destination only grows. A mistake at the source (a deleted folder, a bad rename) never
  spreads to the copy. A user who wants to tidy the destination does it there by hand.
- Copy-only sync needs no delete path in any `Transport` and no "mirror" mode to guard.
- Renames at the source leave duplicates at the destination under the old name. Accepted, for
  the same reason.
- Distributing later needs a new ADR, which covers code signing, the update channel and the docs
  for users.
