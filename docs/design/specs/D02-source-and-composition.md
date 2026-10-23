# D02 — Source step and transfer composition

- ID: D02 · Status: **Built** · Phase: 2 (Recent sources can come earlier: they need no migration)
- Inspired by: A5, B2 (drop zone with a hint), C1 (Add Files / Add Folders), D5 (folder cards with
  count and size), E2, F5, G5 (storage by type), E6, F6 (quick access / source tiles), D15
  (shortcuts for things you forget)
- Layers touched: `apps/desktop`; `crates/model` (Category table, view types); `crates/core`
  (category at analysis; `sources.rs`: the quick check of chosen sources and the nested-source
  guard in `create_job`); `crates/store` (migration **M-category**, two queries); `crates/ipc`
  (`recent_sources`, `job_categories`, `inspect_sources`). Later (see "As built"): pinned sources
  (store migration 9), mounted disks and folder sizes (`crates/scanner`, `core/src/sizes.rs`),
  and a source that can't be read pausing the job (`core/src/batch.rs`, ADR 0026)
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

Empty:

```
What do you want to send?  (?)
Add as many files and folders as you like; they all go to the same place. filebridge only
reads them: nothing at the source is moved, renamed or deleted.

┌ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ┐
            ⤓  Drop files or folders here
        [ + Choose folders ]   [ + Choose files ]
          Any size, from a single file to a whole disk.
└ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ┘

Recently sent from
┌──────────────────────────┐ ┌──────────────────────────┐ ┌ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ┐
│ 📁 Documentos         +  │ │ 📁 Fotos              +  │   ? Scans
│    /home/demo            │ │    /home/demo            │     /media/demo/disco-antigo
│    to Disco externo · 1d │ │    to Nextcloud · 2 w    │     Not found
└──────────────────────────┘ └──────────────────────────┘ └ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ┘
─────────────────────────────────────────────────────────────────────────────────────
Nothing chosen yet.                     Add at least one file or folder…   [ Next → ]
```

With items:

```
┌ ⤓ Drop more here, or                                       [+ Folders] [+ Files] ┐
┌──────────────────────────────────────────────────────────────────────────────────┐
│ 4 items chosen                                 [Remove the duplicate] [Clear all] │
│ 📁 Fotos   /home/demo          412 items · changed 2 d ago                     ✕ │
│ 📁 2023    /home/demo/Fotos    Already inside Fotos: it would be sent twice. Remove ✕ │
│ 🔗 Atalho  /home/demo          A shortcut (link) to /home/demo/Fotos… Use Fotos instead ✕ │
│ 📄 relatório.pdf  …/Documentos 39 MB · Documents · changed 1 w ago              ✕ │
└──────────────────────────────────────────────────────────────────────────────────┘
Recently sent from   (one row, at most 3)
───────────────────────────────────────────────────────────── (sticky) ─────────────
4 items · 2 folders, 1 file · 39 MB in files │ Removed Scans. [↶ Undo]
                                              Fix 2 items to continue   [ Next → ]
```

- **One lead line** says the source is only read; it is not repeated elsewhere on the page. The
  former four-point "How it works" box, which repeated the stepper, is replaced by the guided
  tour below.
- **Drop zone** (`FileDropZone`): OS drop plus two pickers. Once something is chosen it shrinks
  to a one-line `compact` bar so Next stays near. While files are dragged over the window, a
  full-window `DropOverlay` says "Drop to add": the whole window takes the drop.
- **Chosen** (`SourceList`): each row has a kind icon (`PhFolder`, or the Category icon for a
  file), the name in bold (full path in its tooltip), the parent path muted (a long one loses its
  start, not its end), and details from `inspect_sources`:
  - a folder: its direct items ("412 items", "1,000+ items" past the cap, "Empty folder");
  - a file: its size and Category;
  - both: "changed 3 d ago".

  While the check runs, the details show a skeleton. Past 100 rows a "Show 100 more" link
  appears. ✕ removes a row from the list (nothing is deleted), and focus moves to the row that
  takes its place.
- **Problems**, each with its icon, a sentence and a one-click fix:

  | Problem | Says | Fix |
  |---|---|---|
  | `missing` | Not found: moved, renamed, or on a disk that isn't plugged in. | Remove |
  | `unreadable` | Can't be read with your permissions. | Remove |
  | `symlink` | A shortcut (link) to {target}: links aren't sent. | Use {target} instead (replaces it in place) |
  | `root` | A whole disk's top folder can't be sent; choose the folders inside it. | Remove |
  | `special` | Not a regular file or folder (a device or a pipe). | Remove |
  | covered | Already inside {name}: it would be sent twice. | Remove; or "Remove the N duplicates" in the list head |

  Every one of them **blocks** the step: Next and the stepper's later steps stay disabled, and
  the footer says "Fix N items to continue". A failed check doesn't block, since the analysis
  looks at everything again.
- **Recently sent from** (`RecentSources`): cards in an auto-fill grid (up to 6; one row of 3 once
  something is chosen). Each card shows the folder name, its parent, the destination it last went
  to and a `RelativeTime`. Clicking adds the path. A path that no longer exists is dimmed and
  dashed, says "Not found" and can't be added. The section is hidden when there are none.
- **Sticky footer**: an `aria-live` summary ("4 items · 2 folders, 1 file · 39 MB in files"),
  "Removed X. [Undo]" for 8 s after any removal (Clear all and Remove duplicates included), the
  reason Next is disabled, and Next.

#### Guided tour

The first time step 1 opens, a short guide points at the parts of the page, one at a time:
the stepper → the drop zone → Recently sent from (only if shown) → Chosen (only if shown) →
Next.

- **Layers**: a spotlight (the target's rectangle with `box-shadow: 0 0 0 9999px var(--overlay)`
  and an accent outline) and a card next to it. The card has the step count and dots, a title, one
  sentence, "Don't show guides automatically", and Skip / Back / Next (Done on the last step).
  The card flips to the other side when it doesn't fit, and on a narrow window it sits at the
  bottom (or the top), full width.
- **Ending it**: Done, Skip and Esc all mark the guide's version as seen
  (`localStorage` `filebridge.settings`.`guides.seen`), and so does leaving the page. It never
  starts by itself again unless its `version` is bumped after it changes.
- **Replay**: the `?` next to the title (`TourButton`) plays it again. Settings → "Show guides on
  first use" turns the automatic start off; "Show guides again" clears what was seen.
- It starts only while the service is connected and no other guide is open.
- The guides are data (`src/lib/tour.ts`: target `data-tour` value, title and body keys,
  placement, optional), so another screen gets one without new code.

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
  step 1 is shown, since disks get mounted and unmounted. The chosen sources are checked again on
  focus too.
- `inspect_sources` runs 150 ms after the chosen list last changed (also for a drop on Home, since
  the watch lives in the wizard store); an answer to an older list is dropped.
- `inspect_sources` reads metadata and folder listings only (at most 1001 entries per folder),
  opens a file only to know it can be read, and writes nothing.
- "Already inside" is decided on canonical paths, whole components at a time: `/a/foo` is not
  inside `/a/fo`. The outermost chosen folder wins, and of two identical paths the earlier one
  stays. A source that already has a problem neither covers nor is covered.
- `create_job` refuses nested or repeated sources with the same rule ("… is already inside …"),
  so no client can send a folder twice.
- A path that is already in Chosen is not shown again among the recents.
- The composition is fixed once the analysis has run. Re-analysing (going back and changing the
  sources) recomputes it, because the Category is stored per Entry at analysis time.
- Category is decided by the **source** name's extension, not the corrected destination name. A
  Name rule never changes an extension's meaning, and the source is what the user recognises.

### As built

The three items first left out (pinning, mounted disks, a breakdown by folder) were added later,
with a fourth that mounted disks need:

- **Pinned** (E6): a pin on each Recently-sent card and on each chosen row (not one with a
  problem) keeps a path at hand. Pins are a table of their own (store migration 9,
  `pinned_sources(path, pinned_at)`, in pin order), not a settings list: one row per pin, no
  read-modify-write. `pinned_sources()` answers them with `exists` checked now; `pin_source(path,
  pinned)` takes absolute paths only (it needn't exist: a disk may be unplugged), at most
  `MAX_PINNED_SOURCES` (100). Step 1 shows a **Pinned** grid above Disks and Recently sent from
  (a pinned path isn't repeated among the recents); a pin whose path is gone is dimmed, "Not
  found", and can only be unpinned. Home shows them as a row of buttons that start a transfer
  ([D01](D01-home.md#as-built)). Pins work on phones too.
- **Disks** (F6): `volumes()` lists the disks mounted on the service's computer
  (`filebridge_scanner::volumes`, read-only). Linux reads `/proc/self/mountinfo`: a block device
  (not a snap's loop device, not a network share) mounted under `/run/media/`, `/media/` or
  `/mnt/`, or anywhere outside the system's own folders when sysfs says the disk can be
  unplugged (`removable`, or on a USB bus; an encrypted disk is asked about the device below
  it). udisks, which file managers use, mounts under `/run/media/$USER` or `/media/$USER`, so its
  disks show up without talking to it over D-Bus; a disk plugged in but not mounted isn't
  listed. macOS lists `/Volumes` (not the startup disk's link), Windows the drive letters other
  than the system drive; elsewhere nothing. Free and total space come from `statvfs` on Unix. A
  **Disks** grid in step 1 opens the folder picker on the disk; "Whole disk" adds its mount
  folder (not offered for a drive letter's root, which the source check refuses). The list is
  read again when the window gains focus. Hidden on phones (`canListVolumes()` in
  `lib/platform.ts`).
- **Sizes** (a breakdown by folder): a chosen folder's **Sizes** button opens a side panel
  (`FolderSizesDrawer`). `folder_sizes(scan_id, path)` walks the folder in the service
  (`filebridge_scanner::folder_sizes`: metadata only, links neither followed nor counted, like
  the analysis) and answers files and bytes per item directly inside it, most bytes first (at
  most `FOLDER_SIZES_LISTED` = 200 listed, the rest summed), with the entries it couldn't read
  counted apart. While it runs, `Event::FolderSizesProgress { scan_id, sizes }` carries the
  numbers so far (the first at once, then every 250 ms; advisory: the call's answer is the
  result). `cancel_folder_sizes(scan_id)` stops it, and so does the caller going away; a
  stopped one answers what it counted, `complete: false`. At most 4 run at once. A folder
  inside opens in turn (breadcrumbs go back), stopping the one before; closing the panel stops
  it too. It doesn't use the `entries` query of [D05](D05-files-and-entry-detail.md): it is
  before any analysis, and it doesn't apply the destination's Name rules.
- **A source that can't be read pauses the job** ([ADR 0026](../../adr/0026-unreadable-source-pauses-the-job.md)):
  with disks offered as sources, one can be unplugged mid-send. Before each batch, and for the
  sources of the files that failed, the runner checks each source's root; one that is gone,
  unreadable, or an empty folder while it still has files to send sends the job to `Paused`
  with `error_code: source_unavailable`, its files back to send (not `Failed`, no
  `EntryFailed`). Verification does the same when a source file can't be hashed. Resume carries
  on, or pauses again at once if the disk is still away. The window shows "A source can't be
  read… plug the disk back in, then press Resume" in the job's progress, notifies once, and the
  job log names it; the CLI prints the same advice.

## States

| State | Step 1 recents | Step 1 chosen | Composition |
|---|---|---|---|
| Loading | Section hidden until it answers (it's optional) | Skeleton in each row's details, "Checking…" in the head | Skeleton bar while `Analysing` |
| Empty | Hidden | Big drop zone; footer "Nothing chosen yet." | A job with only directories: "Only empty folders" (they are still created, ADR 0007) |
| Error | Hidden; logged | "These couldn't be checked now; the analysis checks them again." Not blocking | Card replaced by an inline `Notice` with Try again (`analyse` result is still usable) |

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

// As built: not a field of AnalysisSummary but its own operation, `job_categories(job_id)`, so
// the card also works for a job reopened from Transfers (the wizard then has no summary).

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
/// A job's files per Category, most bytes first.
JobCategories job_categories(job_id: String) -> Vec<CategoryCount>;
/// Source roots of past jobs, newest first, one per path; only those that were sent (state
/// past Ready). For the "Recently sent from" list.
RecentSources recent_sources(limit: u32) -> Vec<RecentSource>;
```

Checking the chosen sources (`crates/core/src/sources.rs`):

```rust
pub enum SourceProblem { Missing, Unreadable, Symlink, Root, Special }
pub const SOURCE_CHILDREN_CAP: u32 = 1000; // also exported to TypeScript as a constant

pub struct SourceInfo {
    pub path: String,                      // as given
    pub problem: Option<SourceProblem>,
    pub is_dir: bool,
    pub size: Option<u64>,                 // files
    pub children: Option<u32>,             // folders: direct entries, at most the cap
    pub children_capped: bool,
    pub mtime: Option<i64>,                // unix ms
    pub category: Option<FileCategory>,    // files
    pub covered_by: Option<u32>,           // index of the chosen source that contains it
    pub link_target: Option<String>,       // symlinks that resolve
}
```

```text
/// What each chosen source is and what is wrong with it, without reading contents.
InspectSources inspect_sources(paths: Vec<String>) -> Vec<SourceInfo>;   // protocol 15
```

The host refuses more than 10 000 paths in one call.

Store query for it: `SELECT path, MAX(j.created_at) … FROM job_sources s JOIN jobs j … WHERE
j.state NOT IN ('draft','analysing','awaiting_approval','ready') GROUP BY path ORDER BY 2 DESC
LIMIT ?`, decoding each `path` BLOB and dropping the non-UTF-8 ones.

## i18n

| Key | en | pt |
|---|---|---|
| `source.lead` | Add as many files and folders as you like; they all go to the same place. filebridge only reads them: nothing at the source is moved, renamed or deleted. | Junte os ficheiros e pastas que quiser; vão todos para o mesmo sítio. O filebridge só os lê: nada na origem é movido, renomeado ou apagado. |
| `source.hint` | Any size, from a single file to a whole disk. | Qualquer tamanho, de um só ficheiro a um disco inteiro. |
| `source.problem.*`, `source.covered`, `source.blocked` | see the Problems table above | |
| `tour.*`, `tour.source.*` | the guide's buttons and its five steps | |
| `settings.guides*` | Show guides on first use / Show guides again | Mostrar guias na primeira utilização / Mostrar os guias outra vez |
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
- Recent cards are buttons. A missing one is `aria-disabled="true"` and keeps its tooltip.
- The footer summary is `aria-live="polite"`; "Removed X" is `role="status"`.
- The guide's card is `role="dialog"`, `aria-modal`, labelled by its title and described by its
  sentence. While it is open the page (`#app`) is `inert`, Tab stays in the card, Esc skips,
  ← / → move, an `aria-live` line reads "Guide, step 2 / 5: …", and focus returns to where it
  was. Its motion is off under `prefers-reduced-motion`.

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
7. Choosing `Fotos`, `Fotos/2023`, a link to `Fotos`, a missing path and a file shows the folder's
   item count, the file's size and Category, and three problems; Next and step 2 are disabled
   with "Fix 3 items to continue". "Use Fotos instead", "Remove the duplicate" and Remove each
   clear one, and Undo brings a removal back where it was.
8. `create_job` with `/x` and `/x/y` (or `/x` twice, written differently) is refused; `/x` and
   `/xy` are not.
9. With empty `localStorage` the guide starts on step 1. After Done (or Skip) it doesn't start
   again on reload; `?` plays it, and Settings → "Show guides again" makes it start by itself once
   more.

## Out of scope

- ~~Pinning sources~~, ~~mounted volumes as quick sources~~ and ~~a breakdown by folder~~: built
  later, see [As built](#as-built).
- Counting a folder's whole size in step 1's list. Step 1 shows a file's size and a folder's
  direct items only; the recursive count is done once, in the analysis, with the Name rules of
  the chosen destination. The **Sizes** panel counts on request only.
- Mounting a disk that is plugged in but not mounted (udisks over D-Bus): the file manager does
  that, and the disk then appears.
- Reordering pins by hand.

## Open questions

- Should `json`/`xml`/`yaml` count as *code* or *document*? The proposal is code: in a
  migration of personal data they are almost always project files.
