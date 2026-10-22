# D11 — Destination folder browser

- ID: D11 · Status: **Built** · Phase: 2 (the listing contract is also what Repair destination
  needs in Phase 2)
- Inspired by: F9 (Vault & Vine breadcrumbs INTERNAL › DOCUMENTS › WORK and an item count),
  D5 (folder cards), A3 (back navigation)
- Layers touched: `crates/transport-api` (new method `list` with an `Unsupported` default, cap
  `LIST`), `crates/transport-rclone`, `crates/transport-fake`, `crates/model` (`SendOptions.target_dir`,
  view types), `crates/core` (browse; prefix destination paths; validate the folder name),
  `crates/ipc` (`browse`), `apps/desktop` (`Breadcrumbs`, browser dialog, `ProfileForm`,
  `StepDestination`)
- Depends on: [01](../01-design-system.md) (`Breadcrumbs`, `EmptyState`)

## Problem

- A profile's root is typed by hand in `ProfileForm` (for Local there is a folder picker; for
  SFTP and Nextcloud it is a text field). A typo in a path on a server the user can't see is
  found only at **Test**, or worse, the send creates a new folder in the wrong place (story S11).
- Every transfer to a profile lands in the profile's root. Sending "Fotos" to `demo/Backup/2019`
  one day and to `demo/Backup/2020` the next needs two profiles, or editing the profile in between.
- Repair destination (Phase 2, [04](../../04-name-correction.md)) has to list a remote folder,
  and the `Transport` contract can't do that yet.

## User stories

- S11: choose where on the destination the files land.
- As the person migrating, I want one profile per server, not one per folder.

## Design

### Browser dialog

```
Choose a folder on Nextcloud home
Nextcloud home › demo › Backup › 2019                         12 folders
┌──────────────────────────────────────────────────────────────┐
│ 📁 Fotos                                         modified 3 d │
│ 📁 Documentos                                    modified 1 w │
│ 📁 Trabalho                                      modified 2 m │
│ …                                                             │
│ [+ New folder…]                                               │
└──────────────────────────────────────────────────────────────┘
Files will go into:  demo/Backup/2019/Fotos-novas   (will be created)
                                                [Cancel]  [Choose this folder]
```

- **Breadcrumbs** at the top. Each segment is a button; long paths collapse the middle into "…"
  ([01](../01-design-system.md#components)). The first segment is the profile's name (when
  choosing a per-transfer folder) or the backend's top (when choosing a profile's root).
- Folders only (`dirs_only`), sorted by name with numbers in natural order ("2" before "10"),
  with a count. Files are not listed: this is a place picker, not a file manager.
- Double-click or Enter opens a folder; **Choose this folder** takes the current one.
- **New folder…** asks for a name and appends it as a **pending** segment, shown in italics with
  "(will be created)". Nothing is created at the destination by the browser. The send creates
  the folder like any other parent directory, so on Nextcloud it gets the right owner and is
  scanned like everything else sent ([05](../../05-integration-nextcloud.md)).
- The name typed for a new folder is checked with the profile's Name rules (NFC, forbidden
  characters, reserved names, the Integration's rules). A name the destination wouldn't accept is
  refused inline, with the proposed fix offered: "Nextcloud can't store `Fotos:2019`. Use
  `Fotos_2019`?"

### Where it is used

| Place | Chooses | Stored in |
|---|---|---|
| `ProfileForm` → root field → **Browse…** (SFTP; Local keeps the OS picker) | the profile's root | `Destination.root` |
| Nextcloud profile → folder field → **Browse…** (after detection succeeded) | the folder inside the user's files | the Nextcloud settings' folder |
| Wizard step 2, under the selected profile: **Send into: demo/Backup** [Change…] | a sub-folder for this transfer only | `SendOptions.target_dir` |
| Destinations card → **Browse** | nothing (look only; the Choose button is hidden) | — |

## Behaviour

- Each folder is listed by one `browse` call. Results are cached in the dialog for its lifetime
  (back and forward in the breadcrumbs don't re-list), and **Refresh** re-lists.
- Listing a folder with 10 000 sub-folders: the service caps the answer at 2 000 entries, sorted,
  with `truncated: true`, and the dialog shows "Showing the first 2 000. Type to filter". A
  filter field then narrows the list by substring (client-side over the loaded entries, plus a
  `prefix` argument to the service for the rest).
- `target_dir` per transfer:
  - It is relative to the profile root, `/`-separated and NFC, and each component passes the
    Name rules (refused at `set_options` otherwise, with the rule's message).
  - At Send, core prefixes every Entry's destination path with it (`RemotePath` join). The
    Manifest stores the full relative path, so verification, the report and Repair all see the
    real location.
  - Changing it after the analysis is allowed in `Draft`/`AwaitingApproval`/`Ready`
    (`Op::SetOptions`, as today) and triggers a re-analysis, because collisions depend on what
    is already in the folder only after Repair-style checks. For now it only re-runs the Name
    plan.
- The window never builds paths by string concatenation. The service returns the canonical
  `path` of every listed folder.

### As built

- `BrowseTarget::Draft` carries a whole `Profile` (not only a `Destination`), so a draft
  Nextcloud or SFTP profile browses with its own name rules and transport.
- The target folder prefixes destination paths both at analysis (the Files tab shows them) and
  at Send (`plan_destinations`); changing it on an analysed job re-plans them. It is checked on
  `create_job` and `set_options`: a relative path whose every folder name the destination accepts
  as it is.
- The CLI's `send` gets `--into <folder>`.
- Real rclone passes the listing part of the contract suite (`operations/list`, `dirsOnly`;
  "directory not found" → `NotFound`).

## States

| State | Behaviour |
|---|---|
| Listing | Skeleton rows; breadcrumbs usable |
| Empty folder | `EmptyState` "No folders here" + New folder… |
| `NotFound` | "This folder no longer exists" + Go up |
| `Auth` / `Transient` | Inline `Notice` with the plain-words reason and Try again |
| Transport without `LIST` | **Browse…** buttons are hidden; the text field stays |

## Data and contracts

### Contract: `transport-api`

```rust
bitflags! { … const LIST = 1 << 7; }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteEntry {
    pub name: String,          // one path component, as the destination stores it
    pub is_dir: bool,
    pub size: Option<u64>,     // files only
    pub mtime: Option<i64>,    // Unix ms, when known
}

#[async_trait]
pub trait Transport: Send + Sync {
    // … existing methods …
    /// The entries directly inside `dir` (the destination root when `None`). `NotFound` if
    /// `dir` isn't there. Default: `Unsupported` (adapters advertise `LIST` when they implement it).
    async fn list(&self, dest: &Destination, dir: Option<&RemotePath>, dirs_only: bool) -> Result<Vec<RemoteEntry>> {
        Err(TransportError::unsupported("listing is not supported by this transport"))
    }
}
```

- Same pattern as the `set_bandwidth_limit` default method that the send-window work adds to this
  trait: an existing adapter keeps compiling and behaves as before.
- `transport-rclone`: `operations/list` with `{ fs, remote, opt: { dirsOnly } }` → `list[]` →
  `Name`, `IsDir`, `Size`, `ModTime`. Advertises `LIST`.
- `transport-fake`: `read_dir` on its root. Advertises `LIST`.
- Contract suite: `list(None)` after a transfer contains the top-level names that were sent;
  `list` of a missing dir is `NotFound`; with `dirs_only`, no entry has `is_dir == false`.
- No `make_dir`: the browser never writes (see Behaviour). The contract stays as small as the need.

### Model

```rust
pub struct SendOptions {
    pub verify: bool,
    pub concurrency: u32,
    /// Folder under the profile root that this transfer goes into; `None` = the root.
    pub target_dir: Option<String>,
}

#[derive(..., specta::Type)] #[serde(tag = "kind", rename_all = "snake_case")]
pub enum BrowseTarget { Profile { id: String }, Draft { destination: Destination } }

pub struct RemoteEntryView { pub name: String, pub path: String, pub mtime: Option<i64> }
pub struct BrowseResult {
    pub path: String,                     // canonical path of the listed folder
    pub segments: Vec<String>,            // for the breadcrumbs
    pub folders: Vec<RemoteEntryView>,
    pub truncated: bool,
}

/// Checks a folder name typed in the browser against the target's Name rules.
pub struct FolderNameCheck { pub ok: bool, pub proposed: Option<String>, pub message_key: Option<String> }
```

`BrowseTarget::Draft` lets `ProfileForm` browse **before** the profile is saved. The
`Destination` it carries holds keyring references only, never secrets (ADR 0004), like every
saved profile.

### IPC

```text
/// Folders inside `dir` of a profile's destination (or of a destination being edited).
Browse browse(target: BrowseTarget, dir: Option<String>, prefix: Option<String>) -> BrowseResult;
/// Whether `name` is acceptable as a folder name at this target, and the fix if not.
CheckFolderName check_folder_name(target: BrowseTarget, name: String) -> FolderNameCheck;
```

### Core

- `Engine::browse`: resolves the transport from the `Registry` by the destination's transport id,
  checks `LIST`, calls `list(…, dirs_only = true)`, sorts naturally, caps at 2 000 and applies
  `prefix`.
- `Engine::check_folder_name`: runs the profile's `RuleSet` (general rules + the Integration's
  rules) on a synthetic `Entry` for the name.
- At Send (`run.rs`, where items are built): `dest = target_dir.join(dest_path)` when
  `target_dir` is set; the Manifest's `dest_path` is written with the prefix included.
- Integration `after_transfer` receives the full changed paths, so Nextcloud's scan of only the
  sent files keeps working unchanged.

## i18n

| Key | en | pt |
|---|---|---|
| `browse.title` | Choose a folder on {destination} | Escolher uma pasta em {destination} |
| `browse.count` | {n} folders | {n} pastas |
| `browse.newFolder` | New folder… | Nova pasta… |
| `browse.willCreate` | (will be created) | (será criada) |
| `browse.choose` | Choose this folder | Escolher esta pasta |
| `browse.into` | Files will go into: | Os ficheiros vão para: |
| `browse.truncated` | Showing the first {n}. Type to filter | A mostrar as primeiras {n}. Escreva para filtrar |
| `browse.badName` | {destination} can't store "{name}". Use "{proposed}"? | {destination} não aceita "{name}". Usar "{proposed}"? |
| `step.dest.sendInto` | Send into: | Enviar para: |

## Accessibility

- The folder list is a `listbox` with `aria-activedescendant`: arrow keys move, Enter opens,
  Backspace goes up a level.
- Breadcrumbs follow [01](../01-design-system.md#components) (`nav`, `aria-current`).
- The pending segment says "will be created" to screen readers, not just in italics.

## Acceptance criteria

1. Browsing an SFTP profile lists the root's folders (files excluded), naturally sorted, and the
   breadcrumbs navigate back up without new requests.
2. `ProfileForm` can browse an SFTP destination before it is saved, and the chosen path fills
   the root field.
3. "New folder…" with `Fotos:2019` on a Windows-family or Nextcloud profile is refused with the
   proposal `Fotos_2019`. Accepting it sets the pending segment, and **nothing** is created at the
   destination until a send runs.
4. A transfer with `target_dir = "2019/Fotos-novas"` stores Manifest destination paths prefixed
   with it, verification finds the files there, and Nextcloud's after-transfer scan covers exactly
   those paths.
5. The contract suite passes for `transport-rclone` and `transport-fake` with `LIST`, and an
   adapter without `list` still compiles and hides **Browse…**.
6. A folder with 5 000 sub-folders answers 2 000 with `truncated: true`, and typing a prefix
   finds a folder beyond the first 2 000.

## Out of scope

- Listing or opening **files** at the destination, deleting, renaming or moving there (rejected:
  [review](../00-reference-review.md#rejected-patterns)).
- Creating folders from the browser (see Behaviour).
- Remembering the last-used `target_dir` per profile. It could come later as a suggestion in step 2.
