# 04 — Name correction

Names are corrected **during the send, never at the source**: the original file stays intact and
the destination receives the correct name directly. So there are no renames after verification,
and no risk of a later sync recreating the old name. See [ADR 0005](adr/0005-fix-names-in-transit.md).

## Rules

| Rule (`id`) | Detects | Correction | Automatic? |
|---|---|---|---|
| NFC normalisation (`nfc`) | Decomposed accents (`c` + separate cedilla), typical of macOS, iOS and some backups | Convert to NFC; the visible name doesn't change | Yes |
| Invalid encoding (`encoding`) | Bytes that are not UTF-8 (Latin-1, CP1252) | Decode with the most likely encoding | Yes, with preview |
| Mojibake (`mojibake`) | `Ã³`, `Ã§`, `Ã£` (UTF-8 read as Latin-1) | Reverse the double encoding (the `ftfy` approach) | No: suggestion to approve |
| Forbidden characters (`forbidden-chars`) | `< > : " / \ \| ? *` and control characters (forbidden on Windows and SMB) | Replace with `_` or a Unicode look-alike | Yes |
| Trailing spaces and dots (`trailing`) | Name ending in a space or `.` | Remove | Yes |
| Reserved names (`reserved`) | `CON`, `NUL`, `COM1`… (Windows); `.htaccess`, `.part` (Nextcloud) | Prefix or rename | Yes |
| Collisions (`collision`) | Two names that become equal after normalisation, or differ only in case | Suffix ` (2)` | No: ask for a decision |
| Length (`length`) | Path above the destination's limit | Shorten the middle of the name | No: ask for a decision |

Where each rule lives:
- `crates/names`: general rules — `nfc`, `encoding`, `mojibake`, `forbidden-chars`, `trailing`,
  `reserved` (Windows set), `collision`, `length`.
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

## Mojibake detection (approach)

- Candidate if the decoded name contains sequences typical of UTF-8-read-as-Latin-1/CP1252
  (`Ã` followed by a byte in `0x80–0xBF` range when re-encoded, `â€™`, etc.).
- Fix: encode the string as Latin-1/CP1252 → decode as UTF-8; accept only if the result is valid
  UTF-8 **and** has fewer "weird" characters than the input (the `ftfy` heuristic). Repeat for
  double-mojibake, at most 2 rounds.
- Always a proposal, never automatic — false positives are possible on legitimate names.

## Collision handling

- Key = NFC + case-folded name within the same destination directory (case folding only when the
  target is case-insensitive).
- When two or more source entries map to the same key, all but the first get a proposal with
  ` (2)`, ` (3)` before the extension; the user must confirm.

## "Repair destination" mode

For when the files are **already at the destination** (the real case that motivated this app):
"Repair destination" scans a remote folder through the `Transport`, applies the same rules with
the same preview, renames **on the destination**, then runs the integration update
(`Integration::after_transfer` with the renamed paths). It's a job type of its own
(`JobKind::Repair`) and reuses stages 2, 3 and 6.

## Phase 0 risk

Renaming per file with rclone may be slow for hundreds of thousands of files. Phase 0 measures it.
**Plan B:** send with original names to a temporary folder, rename in batch at the destination,
then move into place. See [09 — Roadmap](09-roadmap.md) and [10 — Risks](10-risks-and-open-decisions.md).
