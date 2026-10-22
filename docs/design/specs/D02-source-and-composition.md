# D02 — Source step and transfer composition

- ID: D02 · Status: Proposed · Phase: 2 (Recent sources can come earlier: they need no migration)
- Inspired by: A5, B2 (drop zone with a hint), C1 (Add Files / Add Folders), D5 (folder cards with
  count and size), E2, F5, G5 (storage by type), E6, F6 (quick access / source tiles), D15
  (shortcuts for things you forget)
- Layers touched: `apps/desktop`; `crates/model` (Category table, view types); `crates/core`
  (category at analysis); `crates/store` (migration **M-category**, two queries); `crates/ipc`
  (`recent_sources`, `AnalysisSummary.categories`)
- Depends on: [01](../01-design-system.md) (`StorageMeter`, Category tokens); the space-fit bar
  uses [D03](D03-destination-storage.md) when it exists and falls back to free space only.

## Problem

- Step 1 (`StepSource.vue`) lists the chosen paths as plain text. Nothing says how big each one is
  until step 3, and nothing helps pick a folder sent before. A repeat send means browsing the
  disk again (stories S13, S12).
- Step 3 (`StepAnalysis.vue`) shows files, dirs, size and skipped symlinks, plus a "too big for
  free space" warning (`tooBig` in that file). It doesn't say **what** is being sent. A wrong
  folder, say 400 GB of videos where documents were meant, is only noticed after the send (S3).
- Free space is one number. How close the send comes to filling the destination isn't visible (S4).

## User stories

- S3: know what a transfer contains before I send it.
- S4: know whether the destination has room.
- S13: add folders I send often without browsing for them.

## Design

### Step 1 — Source

```
What do you want to send?
Drag files and folders onto this window, or use the buttons below. …

┌ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ┐
            ⤓  Drop files or folders here
        [ + Choose folders ]   [ + Choose files ]
   Any size. filebridge only reads them: nothing at the source changes.
└ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ┘

CHOSEN (2)
│ 📁 Fotos           /home/demo                                   ✕ │
│ 📄 contrato.pdf     /home/demo/Documentos                        ✕ │

RECENTLY SENT FROM
[ 📁 Fotos · to Nextcloud home · 2 d ]  [ 📁 Trabalho · to Backup disk · 1 w ]
[ 📁 Scans · to Nextcloud home · 3 w ]  (missing: dimmed, not clickable)
```

- The drop zone keeps today's `FileDropZone` behaviour (OS drop plus two pickers). Restyled per
  [01](../01-design-system.md): dashed `--border-strong`, `PhArrowLineDown`, and a hint line that
  reassures instead of limiting (C2 rejected).
- **Chosen** rows: kind icon (`PhFolder` / Category icon for files), the name in bold, the parent
  path in `.path` muted, and a remove button. Sizes are **not** counted here: counting happens
  in Analysis, once the destination is known ([06](../../06-interface.md) keeps that order, and the
  existing `source.countLater` string says so).
- **Recently sent from**: up to 6 chips, newest first, each with the folder name, the
  destination it last went to and a `RelativeTime`. Clicking adds the path to Chosen. A chip whose
  path no longer exists is dimmed with the reason in its tooltip ("No longer found at
  /media/demo/disk"). The section is hidden when there are none.

### Step 3 — Analysis: "What you're sending"

A new card above the issue groups, shown once the analysis summary arrives.

```
WHAT YOU'RE SENDING                                 25 412 files · 393 GB
████████████████████▒▒▒▒▒▒▒▒▒▒▒▒▓▓▓▓░░
■ Photos & images  18 210 · 212 GB   ■ Videos   312 · 168 GB
■ Documents         6 120 · 9.8 GB   ■ Archives  40 · 2.9 GB   ■ Other 730 · 0.3 GB

DOES IT FIT?                                         Nextcloud home
██████████████████▓▓▓▓▓▓▓▓░░░░░░░░░░░░
■ Already used 1.4 TB   ■ This transfer 393 GB   □ Free after 607 GB of 2.4 TB
```

- **Composition**: `StorageMeter` in `composition` mode, one segment per Category with any bytes,
  ordered by bytes. The legend shows files and bytes per Category (`.num`). Categories under 1 %
  of the bytes are folded into "Other" in the bar but stay listed in the legend.
- **Does it fit?**: `StorageMeter` in `usage` mode. Segments: used (`--border-strong`), this
  transfer (`--info`), free after (`--track`). If the destination reports no total
  ([D03](D03-destination-storage.md)), only free space is known: the bar becomes "this transfer vs
  free space" and the legend says "Total size unknown". If free space is unknown too, the card is
  replaced by one muted line: "This destination doesn't report its free space."
- Over capacity: the "this transfer" segment is drawn up to the end of the bar, the overflow is
  marked with a `--danger` end cap, and the existing `analysis.tooBig` text is shown under it.
  Not enough room is **never** decided from colour alone.
- Close to capacity (free after < 5 % of total, or < 10 GB): a `Notice tone="warn"`: "After this
  transfer, Nextcloud home will have 9 GB free."

## Behaviour

- `recent_sources` is called when step 1 mounts, and again whenever the window gains focus while
  step 1 is shown, since disks get mounted and unmounted.
- A path that is already in Chosen is not shown again among the recents.
- The composition is fixed once the analysis has run. Re-analysing (going back and changing the
  sources) recomputes it, because the Category is stored per Entry at analysis time.
- Category is decided by the **source** name's extension, not the corrected destination name. A
  Name rule never changes an extension's meaning, and the source is what the user recognises.

## States

| State | Step 1 recents | Composition |
|---|---|---|
| Loading | Section hidden until it answers (it's optional) | Skeleton bar while `Analysing` |
| Empty | Hidden | A job with only directories: "Only empty folders" (they are still created, ADR 0007) |
| Error | Hidden; logged | Card replaced by an inline `Notice` with Try again (`analyse` result is still usable) |

## Data and contracts

### Category table

`crates/model/src/category.rs`. The extension → Category mapping is domain data, shared by the
UI, the CLI and the report ([D09](D09-report-export.md)).

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum FileCategory { Image, Video, Audio, Document, Archive, Code, Other }

impl FileCategory {
    /// From a file's name as raw bytes: the text after the last `.`, ASCII-lowercased.
    /// No extension, a non-UTF-8 or longer-than-10-byte extension, or a leading-dot-only name
    /// (".bashrc") → Other.
    pub fn of_name(name: &[u8]) -> Self;
    pub fn as_str(self) -> &'static str;           // "image", … (what the store keeps)
    pub fn parse(s: &str) -> Option<Self>;
}
```

| Category | Extensions |
|---|---|
| image | jpg jpeg png gif webp avif heic heif tif tiff bmp svg psd xcf raw cr2 cr3 nef arw dng orf rw2 raf |
| video | mp4 m4v mov mkv avi wmv webm mts m2ts 3gp mpg mpeg vob flv |
| audio | mp3 m4a aac flac alac wav aif aiff ogg oga opus wma mid midi |
| document | pdf txt md rtf doc docx odt xls xlsx ods csv ppt pptx odp pages numbers key epub |
| archive | zip tar gz tgz bz2 xz zst 7z rar iso dmg |
| code | rs ts tsx js jsx vue py go java kt c h cpp hpp cs rb php sh ps1 sql html css scss json yaml yml toml xml ipynb |
| other | everything else |

The table is data. Changing it changes how **new** analyses classify; stored Entries keep their
Category. Unit tests cover: case-insensitivity (`IMG.JPG`), a double extension (`x.tar.gz` →
archive through `gz`), no extension, a dotfile, and non-UTF-8 bytes.

### Store — migration M-category

```sql
ALTER TABLE manifest_entries ADD COLUMN category TEXT NOT NULL DEFAULT 'other';
CREATE INDEX manifest_category ON manifest_entries(job_id, category);
```

- `NewEntry` gets `category: String` (`store` doesn't depend on `model`, so it keeps a string;
  `core` fills it with `FileCategory::of_name(..).as_str()`). Directories store `'other'` and are
  never counted.
- Existing jobs keep `'other'` for every Entry. The composition card hides itself when every file
  of a job is `other` **and** the job predates the migration (shown as "Not available for
  transfers analysed before this version").
- New query:

```rust
/// Files only, grouped by category; bytes and counts.
pub async fn category_counts(&self, job_id: &str) -> Result<Vec<(String, u64 /*files*/, u64 /*bytes*/)>>;
```

### Core

- `analyse.rs`, where `NewEntry` is built (today with `mtime: None`): set `category` from the
  file name (the last component of `entry.raw_path`).
- `Engine::analysis_summary` adds the counts.

### Model and IPC

```rust
// crates/model/src/views.rs
pub struct CategoryCount { pub category: FileCategory, pub files: u64, pub bytes: u64 }

pub struct AnalysisSummary {
    // … existing fields …
    pub categories: Vec<CategoryCount>,   // ordered by bytes, descending
}

pub struct RecentSource {
    /// Absolute path as the user picked it; only UTF-8 paths are listed (`create_job` takes
    /// `String`s).
    pub path: String,
    pub last_job_id: String,
    pub last_profile_id: String,
    pub last_sent_at: i64,
    /// Checked by the service when asked; `false` = not found now (unmounted disk, deleted).
    pub exists: bool,
}
```

Catalogue (`crates/ipc/src/operations.rs`):

```text
/// Source roots of past jobs, newest first, one per path; only those that were sent (state
/// past Ready). For the "Recently sent from" list.
RecentSources recent_sources(limit: u32) -> Vec<RecentSource>;
```

Store query for it: `SELECT path, MAX(j.created_at) … FROM job_sources s JOIN jobs j … WHERE
j.state NOT IN ('draft','analysing','awaiting_approval','ready') GROUP BY path ORDER BY 2 DESC
LIMIT ?`, decoding each `path` BLOB and dropping the non-UTF-8 ones.

## i18n

| Key | en | pt |
|---|---|---|
| `source.hint` | Any size. filebridge only reads them: nothing at the source changes. | Qualquer tamanho. O filebridge só os lê: nada muda na origem. |
| `source.recent` | Recently sent from | Enviado recentemente de |
| `source.recentItem` | {name} · to {destination} · {when} | {name} · para {destination} · {when} |
| `source.recentMissing` | No longer found at {path} | Já não existe em {path} |
| `composition.title` | What you're sending | O que vai enviar |
| `composition.fit` | Does it fit? | Cabe? |
| `composition.used` | Already used | Já usado |
| `composition.this` | This transfer | Esta transferência |
| `composition.freeAfter` | Free after | Livre depois |
| `composition.tight` | After this transfer, {destination} will have {free} free. | Depois desta transferência, {destination} fica com {free} livres. |
| `category.image` … `category.other` | see [01](../01-design-system.md#file-categories) | |

## Accessibility

- Each `StorageMeter` has a visually hidden table with the same numbers, and the bar itself is
  `aria-hidden`.
- Recent chips are buttons. A missing one is `aria-disabled="true"` and keeps its tooltip
  reachable through `aria-describedby`.

## Acceptance criteria

1. Analysing `/home/demo/Fotos` (fixture: 3 jpg, 1 MOV, 1 pdf, 1 file with no extension) shows
   image 3, video 1, document 1, other 1, with their byte sums matching the scan's total.
2. `FileCategory::of_name(b"IMG_0001.JPG") == Image`; `of_name(b"a.tar.gz") == Archive`;
   `of_name(b".bashrc") == Other`; `of_name(b"x.\xff") == Other`.
3. With a destination reporting total 2.4 TB, used 1.4 TB and free 1.0 TB, and a 393 GB transfer,
   "Does it fit?" shows those three segments and "Free after 607 GB".
4. A transfer larger than free space shows the danger cap **and** the `analysis.tooBig` text.
5. After a job from `/home/demo/Fotos` reaches `Transferring`, step 1 of a new transfer lists
   "Fotos" under Recently sent from. After the folder is renamed on disk, the chip is dimmed with
   the "No longer found" reason.
6. Migrating a database with old jobs leaves them readable; their Analysis/Overview shows the
   "not available for transfers analysed before this version" line instead of an all-Other bar.

## Out of scope

- **Pinning** sources (E6 Favorites / Quick Access): later. The settings table can hold a
  `pinned_sources` list when it is wanted.
- Mounted volumes as quick sources (F6 SD card, external disk): later. It needs a per-OS volume
  listing in the window.
- Counting sizes in step 1, before a destination is chosen. The analysis does it once, with the
  Name rules of the chosen destination.
- A breakdown by folder ("Fotos/2019 is 120 GB"). It could come later on the same `entries` query
  as [D05](D05-files-and-entry-detail.md).

## Open questions

- Should `json`/`xml`/`yaml` count as *code* or *document*? The proposal is code: in a
  migration of personal data they are almost always project files.
