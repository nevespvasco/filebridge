# ADR 0026 — A source that can't be read pauses the job

- Status: Accepted
- Date: 2026-10-23

## Context
Step 1 now offers the disks mounted on the computer as sources (external disks, memory cards;
[D02](../design/specs/D02-source-and-composition.md)). A disk can be unplugged, or a network share
drop, in the middle of a send. Before this, every file left to send from it failed on its own
("no such file or directory"), the job ended `CompletedWithWarnings` with those files `Failed`,
and a file marked `Failed` is not sent again on resume. A whole-call error from the transport
failed the job outright. Neither is the files' fault, and the fix (plug the disk back in) is
outside filebridge, like a full destination ([ADR 0021](0021-full-destination-pauses-the-job.md)).

## Decision
- A **source is unavailable** when its root can't be read (gone, an I/O error, no permission)
  or is a folder that is now empty while it still has files to send (the mount folder of a
  disk that isn't mounted). Checked by `core` on the source's root, without reading its files.
- The transfer stage checks the sources a batch sends from **before** the transport call, and,
  after it, the sources of the files that failed (and of a whole-call error that isn't
  retryable). Files of an unavailable source go back to `Partial` (not `Failed`), emit no
  `EntryFailed`, and the job moves to **`Paused`** with the error code `source_unavailable`
  (`JobErrorCode::SourceUnavailable`) and the reason as its message. The verify stage does the
  same when a source file can't be read for hashing.
- Resume carries on: the batch's check runs again first, so a resume with the disk still
  unplugged pauses again at once and loses nothing.
- The window words it in pt and en ("A source can't be read… plug the disk back in, then press
  Resume"), notifies once like a full destination, and the job log names it.

## Consequences
- 07's rule extends: a missing **source** is resumable, not `Failed`. A missing destination and
  revoked credentials still fail the job.
- A single-file source deleted by the user also pauses the job (its root is gone); Cancel ends
  it. Pausing is the safe reading: it never marks files `Failed` that could still be sent.
- One `metadata` and one `read_dir` of each source root per batch, and again for the failed
  files' sources: negligible next to the transfer.
- An empty source folder with nothing left to send never pauses anything: only sources with
  files in the batch are checked.
