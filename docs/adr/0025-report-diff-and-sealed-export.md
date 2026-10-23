# ADR 0025 — Reports are compared by source path; a sealed export is a SHA-256 chain, not a signature

- Status: Accepted
- Date: 2026-10-23

## Context
A job's report ([D09](../design/specs/D09-report-export.md)) is the proof that a migration
worked. Its spec left two things for later: comparing two reports ("what changed since the last
send", before and after a Repair), and a report that shows any edit made after the export. The
user is one person migrating their own files (ADR 0018). A signature would need a signing key:
generated, kept in the keyring, backed up, and its public half carried to wherever the report
is checked. Asked, the user chose a plain digest with no key.

## Decision

**Comparing.** Two finished jobs' reports are compared file by file, **matched by the source
path's exact bytes** (the identity a manifest keeps; directories are left out). Each file that
differs gets one or more kinds: `added`, `removed`, `changed` (size, or SHA-256 when both
reports have one), `renamed` (another destination path: a name rule, or another target folder;
the second report's rules are shown), `newly_verified`, `new_problem`. Both jobs are walked in
source-path order a page at a time (a new index, `manifest_path`), so two 300 000-entry jobs are
compared without holding either. The answer carries the counts per kind over everything, the
first `limit` changes (of one kind when asked), and whether the two jobs have the same sources
and destination: different sources aren't refused, only flagged. In the app: **Compare with…**
in a transfer's menu; in the CLI: `filebridge report diff <a> <b>`, where each side is a job or
an exported JSON report (read whole: the CLI's case).

**Sealing.** A third export format, `sealed`, writes two files: the JSON report and a readable
HTML copy beside it (same name, `.html`). The JSON carries a SHA-256 digest chain:

```text
h0    = SHA-256("filebridge-seal/1\n" ‖ canonical(header))   header: every top-level member but
                                                               "entries" and "seal"
hi    = SHA-256(h(i-1) ‖ canonical(entry i))                  entry i without its "chain";
                                                               "chain": hex(hi) in each entry
final = SHA-256(hn ‖ "\n" ‖ n)
"seal": {"scheme": "filebridge-seal/1", "header": hex(h0), "entries": n, "sha256": hex(final)}
```

`canonical` is the JSON text with keys sorted and no whitespace, so a tool that only reorders or
reformats the file doesn't break the seal; any change of content does. Each link is checked
against the one stored before it, so `filebridge report check` (and **Check a report…** in the
app) names the entry that changed, or the header, the end of the file, or the seal itself.

The HTML copy is rendered **only from what the JSON holds**, deterministically; the check
renders it again and compares byte for byte. Its layout is part of the scheme: a new layout is
`filebridge-seal/2`, and a checker keeps every scheme it ever wrote. The final SHA-256 is shown
after the export, in the HTML and by the check, with the advice to keep it elsewhere.

## Consequences
- An accidental edit, a truncated copy or a corrupted entry is found and pinned to its place.
  Someone who edits on purpose can recompute every digest: against that, only the final SHA-256
  kept somewhere else proves the files unchanged. If that ever isn't enough, a signature over
  `final` can be added as `filebridge-seal/2` without changing the chain.
- No key to generate, store, back up or distribute; nothing new in the keyring.
- CSV and plain JSON exports are unchanged. The sealed JSON is a superset of the plain one
  (`chain` per entry, `seal` at the end), so readers of `filebridge-report/1` still read it.
- One more index on `manifest_entries` (store migration 11): a little more space and insert time
  per scanned entry, for a comparison that never sorts a whole manifest.
- A diff matches by source path: a Repair job (whose "sources" are destination names) compares
  with another Repair of the same folder, not with the send that put the files there.
