# ADR 0033 — A file that keeps failing is retried three times, then given up on and listed

- Status: Accepted
- Date: 2026-10-24

## Context
A file's send can fail on its own while the rest arrive: permission denied at the destination, a
name the server refuses, a timeout on one big file. Until now:

- a **retryable** item error sent the file back to `Partial` and the whole transfer stage backed
  off and ran again, **forever** (docs/07, "Backoff schedule"). One file that always times out
  kept the job in Retrying for good, and the stage stopped at that batch, so the files after it
  waited for every back-off;
- a **non-retryable** item error marked the file `Failed` at once, with no second try.

The user wants a simpler rule: try each failed file again up to three times; if it still fails,
skip it, carry on, and list at the end what couldn't be sent. It also answers D04's open question
"skip this file" for the automatic case.

The risk is a destination that drops off mid-send: every file in flight then fails, and counting
those against each file would give up on files that are fine.

## Decision
- Each manifest entry counts its failed sends in **`manifest_entries.send_failures`**
  (migration 15), persisted in the same statement that sets its state.
- A file that fails (retryable or not) goes back to `Partial` and is retried; after
  **`EngineOptions::max_file_retries` = 3** retries (four sends) it is `Failed`, with the last
  error kept in `error`.
- A failure counts against the file **only when the destination answers**: the batch sent some
  other file, or the transport's `preflight` succeeds. Otherwise the batch counts against no file
  and the stage backs off as before (forever, until the user cancels).
- A failed file doesn't stop the stage: the other batches go first, then the stage backs off
  (docs/07 schedule) and sends the failed files again. So retries are spaced 30 s, 1 min, 2 min.
- A file without a destination name (stage 3 couldn't map it) is still `Failed` at once: no
  retry can fix it.
- At the end the job is CompletedWithWarnings ("N files could not be sent, even after 3
  retries"), and the result card lists the first ten files that weren't sent, each with its
  error, plus how many more (the Files tab's Problems filter has all of them).

## Consequences
- One stubborn file no longer holds the job in Retrying; the job finishes, with the file listed.
- A non-retryable error now costs three more tries (about 3½ minutes of back-off) before the job
  can finish.
- `EntryView` carries the entry's `error`, so any list of files can say why one failed.
- Resending the failed files is still a new transfer or Send again (D06); there is no per-file
  "retry these" action.
