# ADR 0017 — A full destination pauses the job; free space is watched during a send

- Status: Accepted
- Date: 2026-10-23

## Context
[07](../07-security-reliability.md#backoff-schedule) put "disk full" with the non-retryable
errors that move a job to `Failed`. In practice nothing recognised a full disk: rclone's
message became `ErrorKind::Other` (WebDAV's `507 Insufficient Storage` included), so each file
failed on its own, the batch went on uploading file after file to a destination with no room,
and the job ended `CompletedWithWarnings` with every remaining file `Failed` and rclone's
wording as the only explanation. A file marked `Failed` is not sent again on resume.

A full disk is not the files' fault, and it is fixed outside filebridge (delete something,
raise a Nextcloud quota). After that, the same job should simply carry on.

OpenSSH's sftp-server answers a write to a full disk with a bare `SSH_FX_FAILURE` ("Failure"),
so for the most common destination the message alone can't tell.

## Decision
- `transport-api` gains **`ErrorKind::Full`**: no room at the destination. Adapters map to it:
  rclone from WebDAV's `507` (read from its status token, like the other statuses) and from the
  system's words for `ENOSPC` / `EDQUOT` and the SFTP `NO_SPACE_ON_FILESYSTEM` /
  `QUOTA_EXCEEDED` codes; Nextcloud's bulk upload from its request's `507`.
- In `core`, a fatal (`Other`) failure of a file **bigger than the free space measured right
  after** (one transport `preflight`, only when a batch has such failures) counts as `Full`
  too. That covers OpenSSH without reading messages.
- A file refused with `Full` goes back to `Partial`, not `Failed`, and emits no `EntryFailed`.
  The batch ends, the transfer stage ends, and the job moves to **`Paused`** with the message
  and an **error code** (`jobs.error_code`, `JobSummary.error_code = destination_full`) the UI
  words itself in pt and en: the destination is full, free space and press Resume. Resuming
  sends the files that didn't fit.
- rclone stops starting new uploads in a batch after its first `Full` answer (the not-started
  files fail with the same error), as it already did for `Auth`.
- **During the transfer stage** `core` measures the destination's free space (the transport's
  `preflight`, one `operations/about`) at the start and every `space_check_every` (2 min), and
  emits `Event::DestinationSpace { free_bytes, left_bytes }`. The window warns when what is
  left to send doesn't fit; the job log records it once (`space_low`). It never stops the job.
  A destination that doesn't report its space shows nothing. The integration's quota
  (Nextcloud `occ user:info`) is not asked during the send: over WebDAV the transport's
  `about` already reads the quota, and in direct mode the quota isn't enforced on writes.

## Consequences
- 07's rule changes: a full disk is resumable, not `Failed`. Auth refusals and a missing
  destination still fail the job.
- `Paused` now has two causes, the user and a full destination; the error code tells them
  apart (a user pause has no error). A paused job is not resumed on startup, so a full
  destination doesn't loop.
- One more transport call per batch that had fatal failures, and one every 2 minutes during a
  send. Both are cheap (`operations/about`) and time out after 60 s like every preflight.
- A file that failed for another reason but is also bigger than the space left is reported as
  "destination full". That is still the right advice: it couldn't be written either way.
