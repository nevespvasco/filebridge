# D09 — Report export

- ID: D09 · Status: **Built** · Phase: 2
- Inspired by: G8 (slothUI "Export Data"), F12 (checksums as proof)
- Layers touched: `crates/core` (report writer), `crates/store` (streaming read), `crates/model`,
  `crates/ipc` (`export_report`), `crates/host` (CLI wiring), `apps/cli` (`report` command),
  `apps/desktop` (export dialog)
- Depends on: [D05](D05-files-and-entry-detail.md) (`EntryStatus`, stored hashes); uses
  [D02](D02-source-and-composition.md) Category and [D06](D06-transfers-list.md) label when present

## Problem

The proof that a migration worked (every file, where it went, under which name, and that its
checksum matched) lives only inside filebridge's SQLite. Nothing can be archived next to the
data, attached to an issue, or compared with a later send. The whole point of the manifest ([ADR
0005](../../adr/0005-fix-names-in-transit.md)) is proof, and today it can't leave the app
(story S9).

## User stories

- S9: keep a file that proves the migration.
- As the person migrating, I want to open the list of renamed files in a spreadsheet.

## Design

Entry points: Transfer detail → Overview → **Export report…**, the Files tab header
([D05](D05-files-and-entry-detail.md)), and the Transfers row menu ([D06](D06-transfers-list.md)).
Available in terminal states only (a running job's report would be a moving target).

```
Export report: "Photos 2019"
Format     (•) CSV — opens in any spreadsheet      ( ) JSON — for scripts
Include    (•) Every file (25 412)   ( ) Only problems (5)   ( ) Only renamed (312)
           [✓] Folders too
Saved as   photos-2019_2026-10-22.csv                          [Choose…]
                                                       [Cancel]  [Export]
```

- The file name is suggested from the label (slugified, ASCII) and the finish date. **Choose…**
  opens the OS save dialog (`plugin-dialog`, already a dependency).
- During export: a progress bar (rows written / total). The dialog can be closed and the export
  carries on in the service.
- Done: a `Toast` "Report saved", with **Show in file manager**.

## Behaviour

### CSV

UTF-8 with a BOM (so Excel detects the encoding), RFC 4180 quoting, `,` separator, `\r\n`
lines. One header block of `# key: value` comment lines, then a header row:

```
# filebridge report
# transfer: Photos 2019
# transfer_id: 7f3c…
# destination: Nextcloud home (sftp, root demo/Backup)
# created: 2026-10-21T21:58:02Z
# finished: 2026-10-22T09:14:02Z
# state: completed
# verify: sha256, 25 412 checked, 0 different, 0 missing
# files: 25 412 · bytes: 393 004 112 384 · filebridge 0.4.0
source_path,source_path_hex,destination_path,size,category,status,renamed_by,sha256,error
Fotos/2019/Verão/IMG_2041.jpg,,Fotos/2019/Verão/IMG_2041.jpg,4182233,image,verified,,9f2c…04bd,
Documentos/Ata reunião.pdf,446f63…,Documentos/Ata reunião.pdf,214112,document,verified,nfc,1a7e…,
```

- `source_path`: relative to the source's parent (as the Manifest stores it), lossily decoded.
- `source_path_hex`: filled only when the raw bytes aren't valid UTF-8 or differ from their NFC
  form, so the exact original name can always be rebuilt.
- `destination_path`: relative to the profile root.
- `status`: the `EntryStatus` word from [D05](D05-files-and-entry-detail.md#status-what-the-user-sees),
  in English (stable for scripts, whatever the UI language).
- `renamed_by`: rule ids separated by `;` (`nfc;forbidden-chars`).
- Some spreadsheets read a leading `#` row as data; the header comment block can be turned off
  with an option for strict CSV readers (out of the first version: see open questions).

### JSON

One object, streamed (never built in memory):

```json
{
  "format": "filebridge-report/1",
  "transfer": { "id": "…", "label": "Photos 2019", "state": "completed",
                "created_at": "…", "finished_at": "…", "options": { "verify": true, "concurrency": 8 } },
  "destination": { "profile": "Nextcloud home", "transport": "rclone", "backend": "sftp", "root": "demo/Backup",
                   "integration": "nextcloud" },
  "counts": { …Counts… },
  "verify": { "algo": "sha256", "rounds": 1, "checked": 25412, "different": 0, "missing": 0, "unverifiable": 0 },
  "warnings": [ … ],
  "entries": [ { "source_path": "…", "source_path_hex": null, "destination_path": "…", "is_dir": false,
                 "size": 4182233, "mtime": null, "category": "image", "status": "verified",
                 "renamed_by": [], "sha256": "9f2c…" } ]
}
```

`format` is versioned. A change that breaks readers bumps it to `/2`.

### What is never in a report

- Secrets of any kind: keyring references, key file paths, passwords.
- The SSH **host and user**. The destination is identified by profile name, transport, backend
  kind and root. A report is meant to be archived or shared, and host details aren't needed to
  prove the copy. (The `Destination` type holds the host. The writer builds its own
  `ReportDestination` from the allowed fields, so a new field on `Destination` never leaks in by
  default.)

### Where the work happens

The **service** writes the file: the Manifest can be 300 000 rows, and the window only ever holds
pages ([06](../../06-interface.md#principles)). The window and the service run as the same user on the
same machine ([ADR 0008](../../adr/0008-service-ipc.md)), so a path picked in the window is
writable by the service. The service writes to `<path>.part`, then renames it into place, so a
crash never leaves a truncated report under the final name.

### As built

- CSV columns: `source_path, source_path_hex, destination_path, is_dir, size, category, status,
  renamed_by, sha256, error`, after a `#` header block with the transfer's name, id, destination
  (profile, backend kind, root), dates, state, totals, version and its warnings.
- The verify summary in the JSON carries the algorithm and the rounds; the per-status counts
  are left to the entries (one pass, no extra query).
- `Engine::write_report` writes to any `Write` (the CLI's stdout); `export_report` wraps it with
  `<path>.part` and a rename. The desktop gets `dialog:allow-save` for the save dialog.
- **Comparing two reports** ([ADR 0025](../../adr/0025-report-diff-and-sealed-export.md)):
  `diff_reports(job_a, job_b, kind, limit) -> ReportDiff`, finished jobs only. Files (not
  folders) are matched by the source path's exact bytes and each difference gets one or more
  kinds: `added`, `removed`, `changed` (size, or SHA-256 when both have one), `renamed` (another
  destination path), `newly_verified`, `new_problem`. Counts are over everything; the list is
  the first `limit` (at most 5 000) changes in source-path order, of one kind when asked.
  `same_sources` / `same_profile` flag a comparison of unrelated jobs instead of refusing it.
  Both jobs are walked a page at a time in path order (`Store::files_by_path`, index
  `manifest_path`, migration 9). The window: **Compare with…** in a transfer's ⋮ menu opens
  *Compare reports* (`/compare/:a/:b?`): two pickers (the second defaults to the next run of the
  same folders, else the one before it, compared forward in time), a swap button, a chip per
  kind (one at a time), and the changes with what each side says. The CLI:
  `filebridge report diff <a> <b>`, each side a job or an exported JSON report; one
  `kind<TAB>path<TAB>detail` line per change, totals on stderr, exit 1 when anything changed.
- **Sealed export** (ADR 0025): `ReportFormat::Sealed` writes the JSON report with a SHA-256
  digest chain (`chain` in each entry, `seal` at the end: scheme, header link, entries, final
  SHA-256) plus a readable, self-contained HTML copy beside it (`<path>` with `.html`, no
  script), both through `.part` files. `ExportResult` gains `readable` and `seal` (the final
  SHA-256), shown after the export with "keep this code somewhere else". A plain digest, no
  signing key (the user's choice). `check_report(path) -> ReportCheck` and
  `filebridge report check <file>` take the JSON or the HTML, check the other one beside it
  too, and name what changed: the header, an entry (by index and path), the end or the seal,
  or the readable copy (rendered again from the JSON and compared byte for byte). Reordering or
  reformatting the JSON's members doesn't break the seal; any content change does. The
  window: **Check a report…** on Transfers (not on phones, which can't pick a file).

## States

| State | Behaviour |
|---|---|
| Path not writable | Error in the dialog: "Can't write to {folder}: {reason}"; the choice stays |
| File exists | The OS save dialog already asked to replace it |
| Job not terminal | Menu item disabled with the reason "Available when the transfer has finished" |

## Data and contracts

### Model

```rust
#[derive(..., specta::Type)] #[serde(rename_all = "snake_case")]
pub enum ReportFormat { Csv, Json, Sealed }
#[derive(..., specta::Type)] #[serde(rename_all = "snake_case")]
pub enum ReportScope { All, Problems, Renamed }
pub struct ReportRequest { pub format: ReportFormat, pub scope: ReportScope, pub include_dirs: bool, pub path: String }
pub struct ExportResult { pub path: String, pub rows: u64, pub bytes: u64,
                          pub readable: Option<String>, pub seal: Option<String> }
// report.rs (ADR 0025)
pub enum ReportChangeKind { Added, Removed, Changed, Renamed, NewlyVerified, NewProblem }
pub struct ReportDiff { counts, unchanged, changes: Vec<ReportChange>, more, same_sources, same_profile }
pub struct ReportCheck { intact, path, readable, entries, sha256, problems: Vec<SealProblem>, more }

// event.rs: progress while writing (advisory)
ReportProgress { job_id: String, rows: u64, total: u64 },
```

### IPC

```text
/// Writes a job's report (terminal states only) to `req.path` on this machine.
ExportReport export_report(job_id: String, req: ReportRequest) -> ExportResult;
/// Two finished jobs' reports compared (ADR 0025). Protocol 25.
DiffReports diff_reports(job_a: String, job_b: String, kind: Option<ReportChangeKind>, limit: u32) -> ReportDiff;
/// A sealed report file on the service's machine checked (ADR 0025). Protocol 25.
CheckReport check_report(path: String) -> ReportCheck;
```

`Op::Export` is allowed in terminal states (rules.rs). The request runs to completion on the
service even if the window disconnects.

### Store

`for_each_entry(job_id, filter, |row| …)`: a streaming read in `idx` order inside one read
transaction, in batches of 1 000 rows, using the same filter builder as `entries_filtered`
([D05](D05-files-and-entry-detail.md#store)).

### Core

`core/src/report.rs`:

- `write_csv(w, header, rows)` and `write_json(w, header, rows)`, testable against a `Vec<u8>`.
- `ReportHeader` is built from `JobSummary` + `Profile` → `ReportDestination` (allowed fields only).

### CLI

```text
filebridge report <job> [--format csv|json|sealed] [--only problems|renamed] [--dirs] [-o FILE]
filebridge report diff <job-or-report.json> <job-or-report.json> [--only KIND]
filebridge report check <report.json|report.html>
```

Without `-o`, it writes to stdout (so `filebridge report 7f3c --only problems | less` works).
It shares `core::report` with the service.

## i18n

| Key | en | pt |
|---|---|---|
| `report.title` | Export report | Exportar relatório |
| `report.csv` | CSV — opens in any spreadsheet | CSV — abre em qualquer folha de cálculo |
| `report.json` | JSON — for scripts | JSON — para scripts |
| `report.scope.all` | Every file ({n}) | Todos os ficheiros ({n}) |
| `report.scope.problems` | Only problems ({n}) | Só os problemas ({n}) |
| `report.scope.renamed` | Only renamed ({n}) | Só os renomeados ({n}) |
| `report.saved` | Report saved | Relatório guardado |
| `report.notTerminal` | Available when the transfer has finished | Disponível quando a transferência terminar |
| `report.sealed` | Sealed — JSON plus a readable HTML copy; any later change shows when checked | Selado — JSON e uma cópia HTML legível; qualquer alteração posterior aparece ao verificar |
| `report.check.*` | Check a sealed report, and what changed | Verificar um relatório selado, e o que mudou |
| `report.compare.*` | Compare reports: added, removed, changed, renamed, now verified, new problem | Comparar relatórios: novo, removido, alterado, renomeado, agora verificado, novo problema |

The report's **content** (status words, column names, the `#` header keys) is always English,
so files are comparable whatever the UI language was.

## Accessibility

- The dialog is the existing modal pattern (focus on the first radio, Esc cancels).
- Progress is a `role="progressbar"`; completion is announced.

## Acceptance criteria

1. Exporting a 300 000-entry job as CSV keeps the service's memory growth under 50 MB and
   finishes in under 20 s on an SSD.
2. The CSV opens in LibreOffice Calc with accents intact (BOM) and one row per entry, plus the
   header row.
3. A non-UTF-8 source name has its `source_path_hex` filled, and decoding that hex gives back the
   original bytes exactly.
4. `grep -i` over a report of a job whose profile uses SFTP with a key file finds neither the host,
   the user, the key path, nor any keyring reference.
5. Killing the service mid-export leaves no file at the chosen path, only `<path>.part`.
6. `filebridge report <job> --format json | jq .verify.checked` prints the same number as the UI.

## Out of scope

- PDF "certificates". The sealed export's HTML copy is the readable form.
- Signing the report. The sealed export is a plain digest chain
  ([ADR 0025](../../adr/0025-report-diff-and-sealed-export.md)); a signature over its final
  SHA-256 can come later as a new seal scheme without changing the chain.
- Comparing a report with a Repair of the destination the files went to: a diff matches by
  source path, and a Repair's sources are destination names.

## Open questions

- Is a `#` comment header acceptable in the CSV, or should the summary go in a sibling
  `.summary.txt`? Proposal: keep the comment header, with an option to drop it, once someone needs
  strict CSV.
