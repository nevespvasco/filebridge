# ADR 0007 — Rename in transit with one `copyfile` per file (plan A)

- Status: Proposed. The real-server benchmark is still pending.
- Date: 2026-10-21

## Context
[ADR 0005](0005-fix-names-in-transit.md) fixes names during the send, so the transport has to
write each file under its mapped (corrected) name. The open risk was speed: with rclone that means
one `operations/copyfile` call per file (plan A), instead of one batched `sync/copy`, for
hundreds of thousands of files. The fallback (plan B) was to send with the original names to a
temp folder, batch rename there, then move into place.

Phase 0 built both in `crates/transport-rclone` (`Strategy::PerFile`,
`Strategy::TempThenRename`). `filebridge bench` compares them with a plain `sync/copy` baseline
on the same tree and checks every destination path afterwards.

## Results so far (simulated)
Local SFTP server with simulated latency. Full method and caveats are in
[benchmarks/phase-0-simulated.md](../benchmarks/phase-0-simulated.md).

| Fixture, RTT, concurrency | plan A / baseline | plan B / baseline |
|---|---|---|
| 2,000 × 16 KiB, ~0 ms, 8 | 2.30 | 0.82 |
| 2,000 × 16 KiB, 2 ms, 8 | 0.94 | 0.77 |
| 500 × 16 KiB, 20 ms, 8 | 0.96 | 0.71 |
| 500 × 16 KiB, 20 ms, 32 | 1.04 | 0.67 |
| 40 × 8 MiB, ~0 ms, 8 | 1.11 | 0.66 |
| 40 × 8 MiB, 20 ms, 8 | 1.19 | 0.68 |

Every run delivered every file under the expected name (0 missing, 0 unexpected).

Plan B also showed a **data-loss hazard** on destinations that treat NFD and NFC as the same name
(APFS, some SMB/ZFS setups). A direct NFD→NFC `sync/move` saw the files as "unchanged" and deleted
them. It needed two renames per node through a unique intermediate name to become safe, which
made it slower still.

## Decision (proposed)
Use **plan A**: `Transport::transfer` sends each file straight to its mapped name with
`operations/copyfile`, with bounded concurrency (default 8, user-tunable).
- `rclone rcd` runs with `--no-unicode-normalization`. Name normalisation belongs to `names`
  only, and rclone compares bytes as-is.
- Plan B stays in the code for now, as `Strategy::TempThenRename`, for comparison. It is removed
  if the real-server run confirms this decision.

The gate is **not** closed yet. This ADR becomes Accepted (or is revised) after
`filebridge bench` runs against the real Nextcloud server with a subset of the real `Backup`
folder, and plan A stays within about 0.8× of the baseline there.

## Consequences
- Renaming in transit costs about the same as not renaming. The source is never touched, and
  nothing at the destination is renamed or deleted after the copy.
- One rc HTTP call per file. The client-side overhead is negligible next to SFTP round trips,
  but progress and resume work per file (`copyfile` skips files that are already identical).
- Without `sync/copy`'s listing, empty source directories are not created. They need an explicit
  `operations/mkdir` pass (Phase 1).
- Non-UTF-8 source names can't be passed through rc's JSON. They are reported and skipped
  until the `encoding` rule lands (Phase 2).
- Throughput over high-latency links depends on concurrency (c8 → c32: +52 % for plan A at
  20 ms RTT). Expose it in the profile.
