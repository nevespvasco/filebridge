# 04 — Name correction

Names are corrected **during the send, never at the source**: the original file stays intact and
the destination receives the correct name directly. So there are no renames after verification,
and no risk of a later sync recreating the old name. See [ADR 0005](adr/0005-fix-names-in-transit.md).

## Rules

| Rule (`id`) | Detects | Correction | Automatic? |
|---|---|---|---|
| NFC normalisation (`nfc`) | Decomposed accents (`c` + separate cedilla), typical of macOS, iOS and some backups | Convert to NFC; the visible name doesn't change | Yes |
| Invalid encoding (`encoding`) | Bytes that are not UTF-8 (Latin-1, CP1252, or another code page) | Decode as CP1252; the five bytes CP1252 leaves undefined become `_` (never a control character) | Yes, with the bytes shown, only when the reading looks like a Latin-script name (see below). Otherwise a decision, with that reading as the proposal |
| Mojibake (`mojibake`) | `Ã³`, `Ã§`, `Ã£` (UTF-8 read as Latin-1) | Reverse the double encoding (the `ftfy` approach) | No: suggestion to approve |
| Forbidden characters (`forbidden-chars`) | `< > : " / \ \| ? *` and control characters (forbidden on Windows and SMB) | Replace with `_` or a Unicode look-alike | Yes |
| Trailing spaces and dots (`trailing`) | Name ending in a space or `.` | Remove | Yes |
| Reserved names (`reserved`) | `CON`, `NUL`, `COM1`… (Windows); `.htaccess`, `.part` (Nextcloud) | Prefix or rename | Yes |
| Collisions (`collision`) | Two names that become equal after normalisation, or differ only in case | Suffix ` (2)` | No: ask for a decision |
| Length (`length`) | Path above the destination's limit | Shorten the middle of the name | No: ask for a decision |

Where each rule lives:
- `crates/names`: general rules. The per-name ones are `NameRule`s — `encoding`, `mojibake`,
  `nfc`, `forbidden-chars`, `trailing`, `reserved` (Windows set). `length` and `collision` need the
  whole tree (the parent's path, the other names), so they are part of the Name plan
  (`NamePlan::propose`) and run last, in that order.
- `crates/integration-nextcloud`: Nextcloud-specific rules — NFC enforced, Nextcloud reserved
  names (`.htaccess`, `.part`, …), any other server-side blacklist.

`applies_to(TargetProfile)` decides which general rules are active (e.g. `forbidden-chars` and
Windows `reserved` only when the target is Windows/SMB or the integration requires it).

## How it works

1. **The scanner walks the source and reads names as bytes**, never assuming UTF-8 (that's how the
   invalid names were found). On Unix use `OsStrExt::as_bytes`; on Windows names are UTF-16 and
   can't be invalid in the same way, but NFD and forbidden-character rules still apply.
2. **Each active rule** (general + those of the chosen integration) produces issues and proposals.
   Rules run in a fixed order — `encoding` → `mojibake` → `nfc` → `forbidden-chars` → `trailing` →
   `reserved` → `length` → `collision` — each one seeing the previous proposal, so `collision` sees
   final names.
3. **The result is a `source → destination name` map**, stored in the manifest
   (`manifest_entries.dest_name`, `issues`).
4. **The UI shows only affected files, grouped by rule**, with before and after visible. For NFC,
   which looks identical on screen, it also shows the bytes (e.g. `63 cc a7` vs `c3 a7`).
5. **The engine sends each file already with the mapped name.**

Directory names go through the same rules; a corrected directory name is applied to every child's
destination path.

## Decisions model

| Severity | Default | UI |
|---|---|---|
| `Auto` | Applied | Listed per rule with a count; user can open the group and override individual items |
| `NeedsDecision` | Pending — blocks Send | Each item needs approve / edit / skip file |

Decisions are persisted per issue, so a resumed job never asks twice.

What each decision means, and when it is checked (the Name plan in `crates/names`):
- **Accept**: use the rule's proposal. Refused where there is none — an undecodable name
  (`encoding` off), or a path no cut can fit (`length`) — so it can only be edited or skipped.
  "Accept all" is offered only when no pending issue of the rule lacks a proposal
  (`RuleCount.no_proposal`), whatever page of the list is loaded.
- **Keep original**: don't apply *this* rule to this name; the other rules still apply. Refused if
  another rule would then change the name instead (Nextcloud's own NFC rule takes over when the
  general `nfc` is kept out).
- **Edit**: the user's name must pass every active rule (no `CON`, `x.part`, NFD on Nextcloud…)
  and be no longer than a name may be.
- **Skip**: the entry and everything under it stay out of the send.

All of these are checked **when the user decides**, one by one or in bulk, and once more at Send
(two entries on one path, or a path made too long by a decision on a folder above, is only known
then). A decision always belongs to its own job.

At Send each name is **worked out again from its decisions**, never taken from what the analysis
stored: the rules not kept out (a rule that waits for a decision applies only where it was
accepted, so a rule added after the analysis changes nothing), then the length cut if accepted,
measured under the parent's final path, then the accepted collision suffix (its number, from the
proposal). So keeping, say, the `mojibake` original under an accepted cut sends the kept spelling
cut, not the repaired one. A name or path still too long is refused unless the user kept its
length (or a folder's above it).

### Encoding: when the CP1252 reading is applied by itself

A name that isn't UTF-8 is read as CP1252. That reading is applied without asking only if every
non-ASCII character is a Latin-1 letter (not `× ÷ Ð ð Þ þ`), a CP1252 letter (`Œ š Ž`…) or
common punctuation (`’ “ ” – — … € « » ° ª º ©`…), with at most two of them in a row, and no byte
is undefined. Bytes from other code pages rarely look like that once read as CP1252 — CP1251
Cyrillic gives runs like `Ïðèâåò`, Shift-JIS and GBK give runs of symbols and accents, CP850
gives `‡` or `‚` — so those wait for a decision (accept the reading, edit, or skip).

## Mojibake detection (approach)

- Candidate if the decoded name contains sequences typical of UTF-8-read-as-Latin-1/CP1252
  (`Ã` followed by a byte in `0x80–0xBF` range when re-encoded, `â€™`, etc.).
- Fix: encode the string as Latin-1/CP1252 → decode as UTF-8; accept only if the result is valid
  UTF-8 **and** has fewer "weird" characters than the input (the `ftfy` heuristic). Repeat for
  double-mojibake, at most 2 rounds.
- "Weird" counts C1 controls and lead+continuation pairs (`Ã³`), and also what a wrong reversal
  lands on: code points that are neither letters, digits, common punctuation nor an accent on a
  letter (unassigned, private use, stray marks), and letters of a script other than the one most
  of the name is in (`Ø¨ test` would become Arabic `ب test`). A name garbled from Cyrillic
  (`ÐŸÑ€Ð¸Ð²ÐµÑ‚` → `Привет`) is still repaired: its letters are all in one script.
- Always a proposal, never automatic — false positives are possible on legitimate names.

## Length

- Limits come from the profile's `TargetSpec` (`max_name_bytes`, `max_path_bytes`): UTF-8 bytes,
  or UTF-16 units on Windows targets (what Windows counts). Paths are measured under the
  destination root, **including the job's target folder** (or the folder a repair works in).
  Defaults: names 255; paths 4096, or 260 on Windows targets (`MAX_PATH`). A profile is refused
  with a name limit below 16 or above 4096, or a path limit shorter than the name limit or above
  65 536.
- A name over the limit, or one that makes its path too long, gets a proposal cut in the middle of
  the stem (`Relatório a…tividades.pdf`): the start, the end and an extension of up to 16 bytes are
  kept, never splitting a character, and no proposal is shorter than 16. Each entry is measured
  under its parent's **proposed** path, so a cut folder shortens its children's paths; a folder
  cut for its path leaves 32 free for the names inside it. When even 16 doesn't fit (the folders
  above already take the room), the issue has no proposal: edit, keep or skip.
- Always a decision: Accept, keep it (the file will then likely fail at the destination), edit or
  skip.

## Collision handling

- Key = NFC + case-folded name within the same destination directory (case folding only when the
  target is case-insensitive; full folding, so `Straße` and `STRASSE` clash). The NFC part matters
  even with the `nfc` rule kept out: a normalising server would store an NFD and an NFC spelling
  as one name.
- When two or more source entries map to the same key, all but the first get a proposal with
  ` (2)`, ` (3)` before the extension, cut first if the suffix would make it too long; the user
  must confirm. A kept collision row says it will be sent as is, so Send waits for one of the two
  to change.

## "Repair destination" mode

For when the files are **already at the destination** (the real case that motivated this app):
"Repair destination" scans a remote folder through the `Transport`, applies the same rules with
the same preview, renames **on the destination**, then runs the integration update
(`Integration::after_transfer` with the renamed paths). It's a job type of its own
(`JobKind::Repair`) and reuses stages 2, 3 and 6.

As built (`crates/core/src/repair.rs`):
- `create_repair_job(profile_id, dir)` needs a transport with `LIST | MOVE`. The folder is kept as
  the job's `target_dir`, so the name plan's paths land under it as a send's land under its
  target folder (and its length counts against the path limit); repair jobs never verify and
  ignore the send window (renames move no data). The folder and options can't change afterwards
  (`set_options` is refused): "Continue" and "Send again" on a repair open its own page, never
  the New transfer wizard, so every rename passes its confirmation. "Send again" checks the same
  folder again.
- Stage 2 lists the folder one level at a time, parents first, up to 500 000 entries. A subfolder
  that can't be read is a scan error (the rest is analysed); not reaching the destination at all
  stops the analysis. On SFTP, symbolic links are skipped (`skip_links`): a link is not renamed
  and nothing outside the folder is reached through it. Stage 3 is the same name plan and
  decisions as a send.
- **Limitation — names that aren't UTF-8 at the destination.** rclone hands them over with the bad
  bytes replaced (U+FFFD), and that spelling can't be asked for again, so such an entry is
  reported as a scan error ("rename it by hand") and left as it is. Reading them as bytes needs
  rclone's `InvalidUtf8` encoding and raw names through `Transport::list`, not done yet.
- The confirmation counts the entries whose name will really change with the decisions as they
  are (`planned_renames`), not the issues.
- Stage 4 renames every entry whose own name changes, **deepest first**, so each rename happens
  under its parent's old path; in one folder, an entry whose new name is another's old name goes
  after that other (a cycle keeps its order: one fails as taken). Each entry's result is saved
  before the next. `Transport::move_path` (`docs/03`) goes through an intermediate name derived
  from the job and the entry (`.fb-repair-<job>-<idx>`), so a rename cut short between its hops
  is finished when the job resumes; it **never overwrites**: a new name that another entry
  already answers to — exactly, in another normalisation or in another case — fails that entry
  alone, checked before the first hop and again right before the second, and the result is
  confirmed by listing. An entry whose old name and intermediate are both gone counts as done
  only if the new name holds the same entry (kind, size, modification time). A second hop that
  fails for good puts the entry back; if even that fails, the warning names where it was left.
- Skip on a folder leaves it and everything inside it as they are.
- Stage 6 gets the folders that hold renamed entries, where they really are (a folder whose own
  rename failed keeps its old name), outermost only, so Nextcloud rescans them and drops the old
  names. The job's warnings count every entry, file or folder, whose rename failed. Progress
  counts files, like the job's counts.
- **Limitation — the destination root.** Repairing the whole destination, the renamed entries at
  its top have no folder to name for stage 6 (`RemotePath` can't be the root), so the
  integration gets the renamed entries themselves: Nextcloud finds the new names, and the old
  ones at the top level go at its next full scan.
- Entry points: Destinations → "Repair names…" (pick a folder), the job log's "Repair these
  names" on an integration problem that asks for it (Nextcloud's incompatible encoding), and
  `filebridge repair --to <profile> [--dir <folder>]`. Renaming waits for a confirmation that
  says where, what changes and what doesn't.

## Phase 0 risk

Renaming per file with rclone may be slow for hundreds of thousands of files. Phase 0 measures it.
**Plan B:** send with original names to a temporary folder, rename in batch at the destination,
then move into place. See [09 — Roadmap](09-roadmap.md) and [10 — Risks](10-risks-and-open-decisions.md).

Phase 0 result so far (simulated): plan A keeps up with a plain `sync/copy`, and plan B is slower and
needed extra care on destinations that treat NFD and NFC as the same name. See
[ADR 0007](adr/0007-rename-in-transit-strategy.md) (Proposed).
