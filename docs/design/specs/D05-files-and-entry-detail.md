# D05 — Files of a transfer and Entry detail

- ID: D05 · Status: **Built** · Phase: 2 (it is the visible half of the roadmap item "Manifest with
  hash and verification independent of names")
- Inspired by: A6, A7, A8 (slothUI table, count badge, search and filter), D6, D7, D9 (UpLink type
  pills, context menu, filter panel), F12 (Vault & Vine File detail with checksum), F15 (type
  chips), D16 (search and filter)
- Layers touched: `crates/store` (queries, migration **M-entry-index**, verify writes the hash),
  `crates/core` (`run.rs` verify stage, an entry view builder), `crates/model` (view types),
  `crates/ipc` (`entries`, `entry_facets`, `entry`), `apps/desktop` (Files tab, `Drawer`), Tauri
  `plugin-opener`
- Depends on: [01](../01-design-system.md) (`DataTable`, `FilterChips`, `SearchField`, `Drawer`,
  `CategoryBar`); [02](../02-information-architecture.md#transfer-detail-tabs) (tabs);
  [D02](D02-source-and-composition.md) for Category (optional: without it every file is Other)

## Problem

- The Manifest has one row per Entry, with the raw source path, size, destination path, transfer
  and verify state and an error. It is the proof that the migration worked, but the window can't
  show it: no operation reads it, and Job detail shows counts only (docs/06 promised a manifest
  view).
- After a send with 3 failures and 2 mismatches there is no way to see **which** files they were,
  except the live `EntryFailed` events received while the window happened to be open.
- The verify stage computes the source SHA-256 (`sha256_file` in `crates/core/src/run.rs`),
  compares it with the destination's, and **throws it away**. The `hash` and `hash_algo` columns
  of `manifest_entries` stay empty. Nothing can show "this file arrived intact, here is its
  checksum" (story S5).

## User stories

- S5: find one file in a finished transfer and see where it went, under which name, and its
  checksum.
- As the person migrating, I want to list only the files that failed or don't match, so I know
  exactly what to look at.
- As the person migrating, I want to see every file whose name was changed in transit.

## Design

### Files tab (Transfer detail → Files)

```
Files  25 412                                                  [⤓ Export report]
[🔍 Search names…                    ]                          Sort: [Folder order ▾]
(All 25 412) (Problems 5) (Verified 25 380) (Renamed 312) (Skipped 27) (Not yet sent 0)
(Photos 18 210) (Videos 312) (Documents 6 120) (Archives 40) (Other 730)
┌──────────────────────────────────────────────────────────────────────────────┐
│ Name                          Folder                       Size   Status        │
│▌IMG_2041.jpg                  Fotos/2019/Verão           4.1 MB   ✓ Verified    │
│▌Ata reunião.pdf  Aa           Documentos/2019            210 kB   ✓ Verified    │
│▌VID_0007.mov                  Fotos/2019/Verão           2.3 GB   ≠ Different   │
│▌Scan 03?.tiff → Scan 03_.tiff Scans                       18 MB   ✗ Failed      │
│ …                                                                               │
└──────────────────────────────────────────────────────────────────────────────┘
```

- The toolbar is always visible (no hidden filter panel, D9 adapted). It has a `SearchField`,
  **status chips** with counts, **Category chips** with counts, and a sort selector. Status and
  Category chips combine with AND; several chips within one group combine with OR.
- Columns: **Name** (`CategoryBar`, the destination name, `PhTextAa` when renamed with the
  source name in a tooltip), **Folder** (destination folder, `.path`, ellipsis in the middle),
  **Size** (`.num`, right-aligned), **Status** (badge = icon + word). Directories are hidden
  unless the chip "Folders" is on (off by default).
- The tab's badge (in the tab bar) is the Problems count.
- Each row has a `RowMenu`: **Details**, **Show source in file manager**, **Copy source path**,
  **Copy destination path**. Nothing else: no Rename, Delete or Download ([rejected
  patterns](../00-reference-review.md#rejected-patterns)).

### Status: what the user sees

Status merges the two Manifest states into one word:

| Status | `transfer_state` | `verify_state` | Icon / tone |
|---|---|---|---|
| Not yet sent | pending | — | `PhCircle` / muted |
| Sending | partial | — | `PhArrowRight` / accent |
| Sent | done | pending (verify off, or not reached yet) | `PhCheck` / ok |
| Verified | done | done | `PhSealCheck` / ok |
| Different | done | mismatch | `PhNotEquals` / danger |
| Missing at destination | done | missing | `PhQuestion` / danger |
| Couldn't verify | done | failed | `PhWarningCircle` / warn |
| Failed | failed | — | `PhXCircle` / danger |
| Skipped | skipped | — | `PhProhibit` / muted |

"Problems" = Different + Missing + Failed + Couldn't verify. The mapping lives in `crates/model`
(`EntryStatus::from_states`) so the CLI and the report ([D09](D09-report-export.md)) use the same
words.

### Entry detail (drawer)

Opened by a row click, Enter on a focused row, a click on an in-flight row ([D04](D04-transfer-queue.md)),
or a deep link `?tab=files&entry=<idx>`. It follows F12's "Parameters" layout.

```
┌ Entry ─────────────────────────────────────────────── ✕ ┐
│        ▣ (Category icon, 40px, Category colour)          │
│        Ata reunião.pdf                                   │
│        Document · 210 kB · ✓ Verified                    │
│                                                          │
│ NAME                                                     │
│   At the source   Ata reunião.pdf   (NFD bytes)           │
│   At destination  Ata reunião.pdf   (NFC)                │
│   Changed by      Accents composed (NFC) — accepted       │
│   [BytesDiff …]                                          │
│                                                          │
│ WHERE                                                    │
│   Source          /home/demo/Documentos/2019/Ata …   ⧉    │
│   Destination     demo/Backup/Documentos/2019/Ata …  ⧉    │
│                                                          │
│ PARAMETERS                                               │
│   Size            214 112 bytes                          │
│   Modified        2019-03-14 10:22                       │
│   Category        Document (.pdf)                        │
│   Transfer        Done                                   │
│   Verification    Same at both ends                      │
│   SHA-256         9f2c 1a7e … 04bd                   ⧉    │
│                                                          │
│ [📂 Show source in file manager]                          │
└──────────────────────────────────────────────────────────┘
```

- **Name** shows the source and destination names, the Name rules that changed it (from the
  Entry's Issues, with each Decision), and the existing `BytesDiff` when the bytes differ.
- **Where**: absolute source path (lossy decoded; when the raw bytes aren't valid UTF-8, a
  `warn` badge "Name isn't valid text: shown approximately" with a hex row underneath) and the
  destination path including the profile's root. Each has a copy button (`Toast` "Path copied").
- **Parameters**: the exact byte count (F12), mtime when known, Category and extension, transfer
  and verify state in words, the error message when Failed, and the **SHA-256** in `.hash` with
  copy. The hash is shown in groups of 4 hex digits for reading; the copy button copies the plain
  64-char hex.
- When there is no hash: "Not computed: this transfer ran without verification" (verify off), or
  "Not computed yet" (pending).
- **Show source in file manager** opens the OS file manager at the file. It is the only action
  that leaves the app, and it reads only.

## Behaviour

- **Paging**: keyset, 200 rows per request; the next page loads when the last 50 rows come into
  view ([01 — Long lists](../01-design-system.md#long-lists)).
- **Search**: case-insensitive substring on the **destination path** and on the **source path**
  (both relative). Debounced 250 ms. The query is NFC-normalised in the window before it is sent,
  so typing "Verão" matches regardless of how the keyboard composed it. Search is
  accent-*sensitive* ("verao" doesn't match "Verão"); see open questions.
- **Sort**: "Folder order" (scanner order = `idx`, the default; folders read as a tree), "Largest
  first" (`size DESC, idx`), "Problems first" (status rank, then `idx`).
- **Live**: while the job runs, the open page re-queries its current window when `EntriesDone` or
  `VerifyProgress` arrive, at most once every 2 s. Rows don't jump: in Folder order, statuses
  change in place.
- The filters live in the route query (`?tab=files&status=problems&cat=video&q=…&sort=size`),
  except `q`, which stays in memory (a search term may be a personal name; see
  [02](../02-information-architecture.md#navigation-map)).

## States

| State | Behaviour |
|---|---|
| Job never analysed (Draft) | Tab disabled with the reason "Files are listed once the analysis has run" |
| Loading | Skeleton rows |
| No match | `EmptyState` "Nothing matches" + Clear filters |
| Job from before this feature (no hashes stored) | Hash row: "Not stored for transfers verified before this version" |

## Data and contracts

### Model

```rust
// crates/model/src/views.rs
#[derive(..., specta::Type)] #[serde(rename_all = "snake_case")]
pub enum EntryStatus { NotSent, Sending, Sent, Verified, Different, Missing, Unverifiable, Failed, Skipped }
impl EntryStatus { pub fn from_states(transfer: &str, verify: &str) -> Self; pub fn is_problem(self) -> bool; }

pub struct EntryFilter {
    pub statuses: Vec<EntryStatus>,        // empty = any
    pub categories: Vec<FileCategory>,     // empty = any
    pub renamed: Option<bool>,
    pub include_dirs: bool,
    pub query: Option<String>,             // NFC, matched as a substring
    pub sort: EntrySort,                   // FolderOrder | LargestFirst | ProblemsFirst
}
pub struct EntryCursor { pub idx: u64, pub size: Option<u64>, pub rank: Option<u8> }

pub struct EntryView {
    pub idx: u64,
    pub is_dir: bool,
    pub name: String,                      // destination name (or source name if unmapped)
    pub dir: String,                       // destination folder
    pub source_name: Option<String>,       // only when different from `name`
    pub size: u64,
    pub category: FileCategory,
    pub status: EntryStatus,
}
pub struct EntryPage { pub items: Vec<EntryView>, pub next: Option<EntryCursor> }

pub struct EntryFacets {
    pub by_status: Vec<(EntryStatus, u64)>,
    pub by_category: Vec<(FileCategory, u64)>,
    pub renamed: u64,
    pub problems: u64,
}

pub struct EntryDetail {
    pub view: EntryView,
    pub source_path: String,               // absolute, lossy
    pub source_path_hex: Option<String>,   // when the lossy text differs from the bytes
    pub dest_path: Option<String>,         // root + relative
    pub mtime: Option<i64>,
    pub hash: Option<HashView>,
    pub issues: Vec<IssueView>,            // existing type
    pub error: Option<String>,
}
pub struct HashView { pub algo: String, pub hex: String }
```

### IPC

```text
/// One page of a job's Manifest, filtered and sorted.
Entries entries(job_id: String, filter: EntryFilter, after: Option<EntryCursor>, limit: u32) -> EntryPage;
/// Counts for the filter chips (whole job, ignoring the current filter).
EntryFacets entry_facets(job_id: String) -> EntryFacets;
/// Everything about one Entry, for the detail drawer.
Entry entry(job_id: String, idx: u64) -> EntryDetail;
/// As built (ADR 0027): a preview of the source image, `None` when it has none.
EntryThumbnail entry_thumbnail(job_id: String, idx: u64) -> Option<EntryThumbnail>;
```

`limit` is capped at 500 by the service.

### Store

- `entries_filtered(job_id, filter, after, limit)`: one SQL statement built from the filter.
  Status is expressed as `(transfer_state, verify_state)` pairs; Category as `category IN (…)`
  (M-category); renamed as `EXISTS (SELECT 1 FROM issues i WHERE i.job_id = m.job_id AND
  i.entry_idx = m.idx AND i.decision IN ('accepted','edited'))`; the query as `(dest_path LIKE ?
  ESCAPE '\' OR CAST(raw_path AS TEXT) LIKE ? ESCAPE '\')` with `%`/`_`/`\` escaped in the input.
- `entry_facets(job_id)`: three `GROUP BY` queries in one read transaction.
- `entry(job_id, idx)` plus `issues_for_entry(job_id, idx)`.

Migration **M-entry-index**:

```sql
CREATE INDEX manifest_verify_state ON manifest_entries(job_id, verify_state);
CREATE INDEX manifest_size ON manifest_entries(job_id, size, idx);
CREATE INDEX issues_job_entry ON issues(job_id, entry_idx);
```

Performance target: on a job of 300 000 entries, the first page of any filter returns in under
150 ms and `entry_facets` in under 300 ms on an SSD (bench in `crates/store/benches` or a
`#[ignore]` test with a generated manifest).

*As built:* the store takes an `EntryQuery` of plain strings (states as `(transfer, verify)`
pairs, Categories as text), so `store` still doesn't depend on `model`; `core/src/files.rs` maps
`EntryFilter` onto it. The engine methods are `files_page`, `entry_facets` and `entry_detail`.
The virtualised list is a `role="grid"` of rows (not a `<table>`), because rows are absolutely
positioned; `J`/`K`/arrows move and `Enter` opens.

### Core: persist the hash (the roadmap's Phase 2 item)

- `run.rs`, verify stage: the new `set_verify_results(job, "sha256", (idx, state, Option<digest>))`
  (built; `set_verify_states` stays for the resets that carry no digest). The source
  digest is written to `hash` with `hash_algo = 'sha256'` for **every** checked entry: Done,
  Mismatch and Missing alike (the source side is known in all three).
- The store's `set_verify_states` updates `hash`/`hash_algo` only when a digest is given (`COALESCE`),
  so a later round without a digest never clears one.
- The algorithm is stored per entry. SHA-256 is the manifest's hash
  ([ADR 0017](../../adr/0017-sha256-for-the-manifest.md)).

### Window

- `views/JobDetail/FilesTab.vue`, `components/EntryDrawer.vue`, `stores/entries.ts` (per-job
  cache of pages keyed by filter).
- **Show in file manager**: add `@tauri-apps/plugin-opener` (Tauri 2 official plugin) and call
  `revealItemInDir(path)`. Capability: `opener:allow-reveal-item-in-dir`, scoped to reveal only
  (no `open-path`, no `open-url`). The window and the service run as the same user on the same
  machine, so the source path is local to the window.

### Thumbnails (as built, ADR 0027)

Added after the first release of this spec (it was out of scope, F13).

- **Engine:** `crates/thumbnail` decodes the local source (JPEG, PNG, GIF, WebP, BMP, TIFF;
  pure Rust `image` codecs), applies the EXIF orientation and shrinks it to fit 256×256 (JPEG,
  or PNG with transparency). Results live in `<cache>/thumbnails/` (OS cache directory, or the
  app's cache on phones), keyed by path + size + mtime, at most 128 MB, least recently used
  removed first; files without a preview leave an empty marker. The source is only opened for
  reading. `core`'s `Engine::entry_source_file` says which local file an Entry is (none for
  folders and Repair jobs); `crates/host` wires the cache into `EngineOperations` and decodes
  at most two at once.
- **Answer:** `EntryThumbnail { mime, base64 }`, or `None` (not an image, video, HEIC/RAW,
  damaged, too large, folder, Repair job, source gone). A service built without the cache
  answers `unsupported`.
- **Files tab:** the Name cell's Category icon sits in a 28 px box (`EntryThumb.vue`); rows of
  Category Image that stay on screen 150 ms ask for their preview, which replaces the icon in
  the same box. `lib/thumbnails.ts` keeps the last 400 in memory and sends at most three
  requests at once, newest first; a row that leaves the screen drops its queued request. A fast
  scroll over 10 000 rows asks only for the rows where it stops.
- **Entry detail:** the preview, when there is one, above the hero (at most 16 rem high), with
  `alt` "Preview of {name}".
- Videos show the Category icon: no dependency-light way to read a frame (ffmpeg, libheif are
  C libraries the phone and desktop builds don't ship).

## i18n

| Key | en | pt |
|---|---|---|
| `files.title` | Files | Ficheiros |
| `files.search` | Search names… | Procurar nomes… |
| `files.chip.problems` | Problems | Problemas |
| `files.chip.renamed` | Renamed | Renomeados |
| `status.entry.verified` | Verified | Verificado |
| `status.entry.different` | Different | Diferente |
| `status.entry.missing` | Missing at destination | Em falta no destino |
| `status.entry.unverifiable` | Couldn't verify | Não foi possível verificar |
| `entry.atSource` | At the source | Na origem |
| `entry.atDestination` | At destination | No destino |
| `entry.changedBy` | Changed by | Alterado por |
| `entry.hashNone` | Not computed: this transfer ran without verification | Não calculado: esta transferência correu sem verificação |
| `entry.reveal` | Show source in file manager | Mostrar a origem no gestor de ficheiros |
| `entry.lossy` | Name isn't valid text: shown approximately | O nome não é texto válido: mostrado aproximadamente |
| `entry.preview` | Preview of {name} | Pré-visualização de {name} |

## Accessibility

- The drawer is `role="dialog"` with `aria-labelledby` on the file name, focus trapped, and Esc
  closing it and returning focus to the row.
- Status badges carry icon + word. The Problems chip count is in its accessible name ("Problems,
  5").
- The hash is read in full by screen readers (no grouping spaces in its `aria-label`).

## Acceptance criteria

1. After a verified send, `manifest_entries.hash` is set (32 bytes, `hash_algo = 'sha256'`) for
   every file entry with `verify_state` in (`done`, `mismatch`, `missing`), and Entry detail shows
   the same hex that `sha256sum` gives for the source file.
2. On a fixture job with 2 failed, 1 mismatch and 1 skipped file, the Problems chip says 3 and
   lists exactly those three. Skipped is not a problem.
3. Searching "Verão" in a job whose source names are NFD returns those files (the query and the
   destination path are both NFC).
4. With 300 000 entries, scrolling the whole list keeps ≤ 120 rows in the DOM and never
   requests more than 500 rows at once.
5. `?tab=files&entry=42` after a reload opens the drawer on entry 42.
6. **Show source in file manager** opens the OS file manager with the file selected, and no
   capability other than `reveal-item-in-dir` is granted to the opener plugin.

## Out of scope

- ~~A thumbnail preview of the source image (F13).~~ Built later: see
  [Thumbnails](#thumbnails-as-built-adr-0027). Video frames stay out (no dependency-light decoder).
- Bulk actions on files (retry these, skip these). The job already retries failures as a whole
  (Resume); per-file actions wait for the skip question in [D04](D04-transfer-queue.md#open-questions).
- Browsing the destination's own files: that is Repair destination ([04](../../04-name-correction.md)).

## Open questions

- **Accent-insensitive search** ("verao" → "Verão") needs a folded copy of each name (an extra
  column filled at analysis, or an FTS5 table with `remove_diacritics`). Worth it for pt users.
  The proposal is FTS5 if search is used a lot after release.
- Should the destination hash be stored too, when it differs? Today only "mismatch" is recorded.
  Storing the remote digest would let the report show both values.
