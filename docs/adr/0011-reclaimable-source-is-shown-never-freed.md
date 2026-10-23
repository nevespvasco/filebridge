# ADR 0011 — A reclaimable source is shown, never freed

- Status: Accepted (2026-10-23)
- Date: 2026-10-22
- Refines: [ADR 0005](0005-fix-names-in-transit.md) ("freeing the source is manual and only
  offered after a zero-difference verification")

## Context
ADR 0005 forbids a "move" mode and says freeing the source is *offered* after a verification
with zero differences. "Offered" can be read two ways: the app deletes the source after a
confirmation, or the app only says it is safe and the user deletes it themselves.

The design review ([docs/design](../design/README.md), shot F's "cleanup scan") brought this to a
head: a screen that shows reclaimable space invites a "Free 381 GB" button. Deleting the source
is the one action in a migration that can lose data for good. A bug in eligibility (a skipped
file, a source changed after the send, a mismatch counted wrong) would then destroy the only
copy.

## Decision
- filebridge **never deletes, moves, trashes or modifies anything at the source**, with no
  exception and no confirmation that unlocks it.
- "Offered" means **shown**: when a job's sources pass the eligibility rules in
  [D10](../design/specs/D10-reclaimable-source.md) (completed, verified with zero differences, no
  skipped files, source re-checked unchanged within 24 h), the app says how much space is
  reclaimable and opens each source folder in the OS file manager. The user deletes it there.
- No code in the engine, service, CLI or window holds a delete path for source files. Eligibility
  is computed by the service and only read by the window.

## Consequences
- The hard rule "never delete or modify the source" stays literal and can be checked by a grep and
  a test, with no carve-out.
- One extra step for the user (deleting in the file manager). This is accepted: it is also a
  moment to look at what is about to be deleted.
- The eligibility rules can be strict (warnings block, unverified jobs block) without risk of
  annoying anyone into a dangerous override, because there is no override.
- Rejected: an in-app "Move to trash" after confirmation. It is safer than a hard delete, but it
  still makes filebridge the actor of a destructive step, and trash behaviour varies per OS and
  per mount (external disks often have no trash).
