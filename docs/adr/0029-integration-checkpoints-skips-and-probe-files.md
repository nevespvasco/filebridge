# ADR 0029 — Integration checkpoints, files a tool skips, and probe files

- Status: Accepted
- Date: 2026-10-23

## Context
Three follow-ups of the Immich integration ([ADR 0020](0020-immich-external-library.md)) did
not fit the contracts:

1. **Waiting for the library scan.** Immich's scan runs in the background; to say when the
   photos are there, the after-transfer step follows Immich's job queue for minutes. If the
   step runs again (a retryable error, or the job resumed after the app stopped), it must not
   ask Immich for a second scan while the first is still running. The step had no place to
   remember that it already asked: `core` persists the job's stage (`Integrating`), not what
   happened inside it.
2. **Files Immich will skip.** A scan passes over files that match the library's exclusion
   patterns, hidden names, and types it doesn't import. The user should hear it before the
   send, but an integration's `preflight` doesn't know the job's files.
3. **Is the SFTP folder the one Immich imports?** The settings say where Immich sees the
   destination (`import_path`), and nothing checked that it is the same folder. The only proof
   is to put something there and ask Immich to find it, then take it away: the `Transport`
   contract could write a file but never remove one.

Phase 3's gate ("a new plugin enters without changing `core`") is about adding a plugin. When
a plugin needs more than the contract gives, the rule in `CLAUDE.md` is to fix the contract,
which is what this does; the contract change touches `core` once, for every integration.

## Decision
- **Checkpoints (`integration-api`).** `IntegrationSink::checkpoint(state).await` hands a small
  JSON state to the host and returns once it is persisted (`true`), or `false` when no host
  keeps it. `JobContext::checkpoint` holds the last one when the step runs again for the same
  job, `None` the first time. `core` stores them in the job's log as `integration_checkpoint`
  events (`Store::last_event` reads the newest) and answers only after the row is written.
  Immich saves `{scan_requested, assets_before}` right after Immich accepts the scan; a crash
  between the two means one more scan, never a lost one.
- **`Integration::will_skip(ctx, files) -> Vec<Problem>`**, default empty: which of the files a
  send is about to write (destination paths) the tool won't take. `core` asks it at the end of
  the analysis, when the names are known and only if the integration's preflight didn't fail,
  and adds one `skipped` warning check to the integration's preflight view (the first few
  paths and the count). It never blocks the send, and it isn't part of the destination's kept
  connection test. Immich lists the same files again as problems of its after-transfer report,
  so the job ends with warnings and the job log has every path.
- **Probe files (`transport-api`).** `PROBE_PREFIX` (`.filebridge-probe-`) and `is_probe` (a
  name with that prefix directly in the destination's root). `Transport::remove_probe` checks
  `is_probe` before touching the destination, then calls `remove_file_unchecked` (default
  `Unsupported`; rclone `operations/deletefile`, the fake `remove_file`). Nothing else can be
  removed through it, so sent files stay safe. The shared contract suite has `check_probe`.
- **Immich uses them:**
  - after the scan request it follows the `library` queue (`GET /api/queues/library`, or
    `GET /api/jobs` on older Immich) with backoff (2 s doubling to 30 s) until it is idle, for
    an hour at most, with progress (assets imported so far). A paused queue is shown as
    waiting. Past the hour, or when the key may not read the queue (`queue.read`, `job.read`
    before Immich 2), the step ends with a problem (`immich.scan-unfinished`,
    `immich.scan-not-followed`), not a failure: the scan was accepted.
  - which files the scan skips: the library's exclusion patterns (globs, case-insensitive,
    a matching folder drops what is inside), a hidden name below the import path, an extension
    not in `GET /api/server/media-types` (Immich's own lists when the server doesn't say;
    `.xmp` sidecars are fine).
  - the wizard writes a probe file into the destination's root with the host's transport, asks
    Immich's library path validation about `<import_path>/<probe>` (Immich answers "Not a
    directory" for a file it finds, ENOENT for one it doesn't), and removes the probe. Not
    found fails the `same-folder` check and nothing is saved; the window explains it in pt/en.
    A folder that doesn't exist yet can't be probed: a warning.

## Consequences
- `crates/core` changed once, for the contract: it persists checkpoints and asks `will_skip`.
  No Immich-specific code is in `core`.
- An integration that ignores the new methods behaves as before (Nextcloud).
- The API key needs `queue.read` (or `job.read`) to follow the scan; without it the send still
  ends, with a warning.
- A crash between Immich accepting the scan and the checkpoint row being written asks for one
  more scan on resume: harmless, it only costs time.
- The probe runs in the profile wizard only, not before every send: it writes to the
  destination, and the folder rarely changes afterwards.
