# ADR 0005 — Fix names in transit, never at the source; never delete the source

- Status: Accepted
- Date: 2026-10-21

## Context
In the manual migration, renaming after verifying confused the verification, and a later rsync
could recreate the old names. Source data must stay safe.

## Decision
Names are corrected **during the send**: the source stays untouched and the destination receives
the corrected name directly. The **manifest** stores `source path → destination name` plus the
source hash, so verification is independent of the final name. There is **no "move" mode**;
freeing the source is manual and only offered after a zero-difference verification.

## Consequences
- No post-verification renames; re-runs don't recreate old names.
- Requires the transport to write each file under a mapped name (performance risk → Phase 0;
  plan B: temp folder + batch rename + move).
- Files already at the destination are handled by the separate "Repair destination" job.
