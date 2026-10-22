# 04 — Name correction

Names are corrected **during the send, never at the source**: the original file stays intact and
the destination receives the correct name directly. So there are no renames after verification,
and no risk of a later sync recreating the old name. See [ADR 0005](adr/0005-fix-names-in-transit.md).

## Rules

| Rule (`id`) | Detects | Correction | Automatic? |
|---|---|---|---|
| NFC normalisation (`nfc`) | Decomposed accents (`c` + separate cedilla), typical of macOS, iOS and some backups | Convert to NFC; the visible name doesn't change | Yes |
| Invalid encoding (`encoding`) | Bytes that are not UTF-8 (Latin-1, CP1252) | Decode as CP1252; a name with one of the five bytes CP1252 leaves undefined is read as Latin-1 | CP1252: yes, with the bytes shown. Latin-1 guess: no, a decision |
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
- **Accept**: use the rule's proposal. Refused on an undecodable name (`encoding`): there is no
  proposal, so it can only be edited or skipped.
- **Keep original**: don't apply *this* rule to this name; the other rules still apply. Refused if
  another rule would then change the name instead (Nextcloud's own NFC rule takes over when the
  general `nfc` is kept out).
- **Edit**: the user's name must pass every active rule (no `CON`, `x.part`, NFD on Nextcloud…).
- **Skip**: the entry and everything under it stay out of the send.

All of these are checked **when the user decides**, one by one or in bulk, and once more at Send
(two entries on one path is only known then). A decision always belongs to its own job.

## Mojibake detection (approach)

- Candidate if the decoded name contains sequences typical of UTF-8-read-as-Latin-1/CP1252
  (`Ã` followed by a byte in `0x80–0xBF` range when re-encoded, `â€™`, etc.).
- Fix: encode the string as Latin-1/CP1252 → decode as UTF-8; accept only if the result is valid
  UTF-8 **and** has fewer "weird" characters than the input (the `ftfy` heuristic). Repeat for
  double-mojibake, at most 2 rounds.
- Always a proposal, never automatic — false positives are possible on legitimate names.

## Length

- Limits come from the profile's `TargetSpec` (`max_name_bytes`, `max_path_bytes`, in UTF-8 bytes,
  measured under the destination root). Defaults: names 255; paths 4096, or 260 on Windows
  targets (`MAX_PATH`).
- A name over the limit, or one that makes its path too long, gets a proposal cut in the middle of
  the stem (`Relatório a…tividades.pdf`): the start, the end and an extension of up to 16 bytes are
  kept, never splitting a character, and no proposal is shorter than 16 bytes.
- Always a decision: Accept, keep it (the file will then likely fail at the destination), edit or
  skip.

## Collision handling

- Key = NFC + case-folded name within the same destination directory (case folding only when the
  target is case-insensitive). The NFC part matters even with the `nfc` rule kept out: a
  normalising server would store an NFD and an NFC spelling as one name.
- When two or more source entries map to the same key, all but the first get a proposal with
  ` (2)`, ` (3)` before the extension; the user must confirm.

## "Repair destination" mode

For when the files are **already at the destination** (the real case that motivated this app):
"Repair destination" scans a remote folder through the `Transport`, applies the same rules with
the same preview, renames **on the destination**, then runs the integration update
(`Integration::after_transfer` with the renamed paths). It's a job type of its own
(`JobKind::Repair`) and reuses stages 2, 3 and 6.

As built (`crates/core/src/repair.rs`):
- `create_repair_job(profile_id, dir)` needs a transport with `LIST | MOVE`. The folder is kept as
  the job's `target_dir`, so the name plan's paths land under it as a send's land under its
  target folder; repair jobs never verify and ignore the send window (renames move no data).
- Stage 2 lists the folder one level at a time, parents first, up to 500 000 entries. Stage 3 is
  the same name plan and decisions as a send.
- Stage 4 renames every entry whose own name changes, **deepest first**, so each rename happens
  under its parent's old path; each entry's result is saved before the next. `Transport::move_path`
  goes through an intermediate name (servers that store NFD and NFC, or two cases, as one name)
  and **never overwrites**: a new name already taken fails that entry alone, reported as a
  warning. A resumed job counts an entry whose old name is gone and new name is there as done.
- Skip on a folder leaves it and everything inside it as they are.
- Stage 6 gets the folders that hold renamed entries (the outermost ones), so Nextcloud rescans
  them and drops the old names.
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
