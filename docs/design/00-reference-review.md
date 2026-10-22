# 00 — Reference review

Seven public designs were studied in full, every image of every shot, including the 14-image
UpLink case study. For each shot this doc lists what it shows, then each feature with a
**verdict**:

- **Adopt**: take it as it is.
- **Adapt**: the pattern fits, but it means something else in filebridge.
- **Reject**: it serves a product filebridge is not (see [Q1](README.md#decisions-this-folder-rests-on)).

Each verdict also names the spec that builds it, or gives the reason it is rejected. The shots
are cited for reference only. No asset, illustration or copy from them is reused.

| Key | Shot | Author | Link |
|---|---|---|---|
| A | slothUI — File Management UIUX | Samuel Oktavianus | <https://dribbble.com/shots/24319955-slothUI-World-s-Laziest-Design-System-File-Management-UIUX> |
| B | Multiple files upload — Upload file modal | Nivedha Raju | <https://dribbble.com/shots/22459100-Multiple-files-upload-Upload-file-modal> |
| C | File Transfer Interface ("Sendora") | Achraf Elkaami | <https://dribbble.com/shots/26855966-File-Transfer-Interface-UI-UX-Design> |
| D | File transfer service ("UpLink" case study) | alisa | <https://dribbble.com/shots/25216429-File-transfer-service> |
| E | CloudVault — Cloud Storage Dashboard | Sudipta Chakrabarti | <https://dribbble.com/shots/26396125-CloudVault-Cloud-Storage-Dashboard-UI> |
| F | Vault & Vine — File Manager App | sleek.design | <https://dribbble.com/shots/27663435-Vault-Vine-File-Manager-App-UI> |
| G | slothUI — File Manager Dashboard ("My Cloud") | Samuel Oktavianus | <https://dribbble.com/shots/24344632-slothUI-Design-System-for-Gen-Z-File-Manager-Dashboard-UI> |

## What filebridge is, as the yardstick

filebridge picks **Sources**, sends them to a **Profile**'s destination with names fixed in
transit, **proves** the copy by checksum, updates the destination's tool, and survives anything in
between. It owns no storage. It never browses, edits, shares or deletes the files it sent, and it
never touches the Source.

Most of the shots are cloud drives: a store that lives on, which you browse, share from and
clean. Patterns that only make sense for a store you own are rejected. Patterns that look like
file-manager features but help someone **see, trust and recover a Job** are kept, usually
pointed at the Manifest instead of at "my files".

---

## A — slothUI: File Management UIUX

**What it shows.** A web app shell: a left sidebar (search; Home, Tasks, Users, APIs,
Subscription, Settings, Help; counter badges; a dismissible "Go Pro" card; the user at the
bottom). A page titled "My Files & Assets" with a "Back to All Projects" link, Share and "Add
Collaborator" buttons, a wide dashed drop zone ("Click here to upload your file or drag",
supported formats, a 10 MB limit), and an "Attached Files" table with an "81 Total" badge,
search, a Filter button, checkboxes, sortable File Name / File Size / Last Modified / Uploaded
By columns, coloured icons per file type, and Delete / Edit links on each row. The design system
claims Phosphor icons and WCAG 2.1.

| # | Feature | Verdict | Where / why |
|---|---|---|---|
| A1 | Sidebar with counter badges | Adopt | Transfers badge for active jobs exists; add an attention dot for jobs that need the user. [02](02-information-architecture.md#sidebar) |
| A2 | Search box in the sidebar (global search) | Adapt | No global search: what can be searched lives in two lists (Transfers, Files of a transfer). A search field per list instead. [D06](specs/D06-transfers-list.md), [D05](specs/D05-files-and-entry-detail.md) |
| A3 | "Back to All Projects" link | Adopt | A back link to Transfers on Job detail. [02](02-information-architecture.md#page-anatomy) |
| A4 | Title with actions on the right | Adopt | Already `.page-head`; formalised in [02](02-information-architecture.md#page-anatomy). |
| A5 | Wide drop zone with a hint line | Adapt | The Source step's drop zone; the hint says what is accepted and that the source is not changed, not a size limit. [D02](specs/D02-source-and-composition.md) |
| A6 | Table with checkboxes, sortable columns and row actions | Adapt | The Files of a transfer table (the Manifest), with no "uploaded by" and no Delete/Edit. [D05](specs/D05-files-and-entry-detail.md) |
| A7 | Count badge next to the table title ("81 Total") | Adopt | [D05](specs/D05-files-and-entry-detail.md), [D06](specs/D06-transfers-list.md) |
| A8 | Search + Filter button over the table | Adopt | As filter chips that are always visible, not a hidden panel. [D05](specs/D05-files-and-entry-detail.md), [D06](specs/D06-transfers-list.md) |
| A9 | Share, Add Collaborator | Reject | No sharing, no other people ([rejected patterns](#rejected-patterns)). |
| A10 | Dismissible upsell card in the sidebar | Adapt | The slot is useful, the content isn't: a **sidebar notice** for low destination space or "the service isn't installed to start on login". [02](02-information-architecture.md#sidebar), [D03](specs/D03-destination-storage.md) |
| A11 | User avatar and logout at the bottom | Reject | Single-user local app. The bottom keeps the service status that exists today. |
| A12 | Coloured icon per file type | Adopt | File **Category** colours and icons. [01](01-design-system.md#file-categories) |

## B — Multiple files upload modal

**What it shows.** A modal, "Upload and attach files", with a one-line subtitle, a light drop
area (formats and a max size), and one card per file: file icon, name, size, a progress bar with
%, a check when done or an ✕ to cancel while sending. Footer: Cancel / Attach files.

| # | Feature | Verdict | Where / why |
|---|---|---|---|
| B1 | Title + one-line subtitle saying what to do | Adopt | Already a principle in [06](../06-interface.md#principles) ("one plain sentence"). |
| B2 | Drop area with its constraints | Adapt | [D02](specs/D02-source-and-composition.md) |
| B3 | A row per file with name, size, bar and % | Adopt | Only for files **in flight** (at most `concurrency`, 8 by default), never for all 300 000. [D04](specs/D04-transfer-queue.md) |
| B4 | Check when done, ✕ to cancel one file | Adapt | Check: the "Just finished" list in [D04](specs/D04-transfer-queue.md). Cancelling one file is **later**: skipping a file mid-send changes the Manifest and needs its own operation ([D04 open questions](specs/D04-transfer-queue.md#open-questions)). |
| B5 | Cancel / primary action footer | Adopt | Exists in the wizard (`.footer`). |

## C — Sendora: File Transfer Interface

**What it shows.** A marketing landing page ("Send large files – Free & secure") with classical
art and a soft gradient. A central card has **Add Files** / **Add Folders** tabs, a dashed drop
area ("Max size 50GB per file or folder", Browse files), "Send to" and "Your email" fields, a
Message field with a Private toggle, a dark full-width "Start Transfer" button, and a "Trusted by
350k+" line with avatars. Partner logos sit underneath.

| # | Feature | Verdict | Where / why |
|---|---|---|---|
| C1 | Add Files / Add Folders as two clear choices | Adopt | Exists as two buttons; kept and restyled. [D02](specs/D02-source-and-composition.md) |
| C2 | "Max size 50GB" | Reject | filebridge has no size limit. The line's job, reassurance, goes to "Nothing at the source changes". |
| C3 | Send to / Your email | Reject | No recipients: the destination is a Profile. |
| C4 | Message | Adapt | An optional **label** for the Job ("Photos 2019 → Nextcloud"), shown in Transfers, Activity and notifications. [D06](specs/D06-transfers-list.md) |
| C5 | Private toggle | Reject | Nothing is shared. |
| C6 | One full-width primary button | Adopt | Exists (`.btn-big` Send in Review). |
| C7 | "Trusted by 350k+" | Reject | Marketing. The reassurance under Send stays factual ("source stays untouched; verify on"). |
| C8 | Art and gradient background | Reject | A tool that runs for 31 hours should look calm, not like a landing page. [01](01-design-system.md#principles) |

## D — UpLink: File transfer service (case study)

**What it shows.** A full case study:

- Research: interviews, personas (developer, journalist), user stories ("know when my team made
  changes", "know how long my file will be retained", "add comments to files", "locate the files
  I need") and four hypotheses: tags and shortcuts for names you forget, comments for feedback,
  search and filter by name/keyword/type/date, and a Bin for accidental loss.
- Wireframes, a palette (#2B6FFC-ish blue, pink, peach, periwinkle) and a grotesk typeface.
- The app: a sidebar (My files, Recent, Groups, Bin, "4.59 GB of 15 GB used" with Buy storage).
  Coloured folder cards with file count and size, and a "+5 More" card. A "My files" table with
  type pills ("• pdf"), Modified, Sharing, Created by and a ⋮ menu.
- A Share modal (mail or group, link permission, Copy link / Send invite) and a 10-item context
  menu: Preview, Share, Manage access, Download, Move to, Copy to, Rename, Details, Version
  history, Delete.
- A Filter panel (Size, Type, Modified, Sharing) with Reset / Apply, and a Bin with Empty bin /
  Restore all items.
- Mobile layouts for every screen.

| # | Feature | Verdict | Where / why |
|---|---|---|---|
| D1 | Research method: user stories, then hypotheses | Adopt | filebridge's own stories are [below](#filebridges-own-user-stories); every spec starts from them. |
| D2 | Sidebar storage meter ("4.59 GB of 15 GB used") | Adapt | Storage of each **destination**, on Destinations and Home. [D03](specs/D03-destination-storage.md) |
| D3 | Recent | Adapt | Recent transfers and Activity on Home. [D01](specs/D01-home.md), [D07](specs/D07-job-log-and-activity.md) |
| D4 | Groups, Buy storage | Reject | No people, no plans. |
| D5 | Folder cards with count and size, "+N more" | Adapt | Destination tiles on Home and Source cards in the Source step. [D01](specs/D01-home.md), [D02](specs/D02-source-and-composition.md) |
| D6 | Table with a type pill, Modified, ⋮ | Adapt | [D05](specs/D05-files-and-entry-detail.md) (Category pill), [D06](specs/D06-transfers-list.md) (row menu). |
| D7 | Context menu (Preview … Delete) | Adapt | Keep **Details**, **Show in file manager** (source side), **Copy path**. Reject Share, Manage access, Download, Move to, Rename, Version history and Delete: each acts on a store filebridge doesn't own, or on the Source. "Copy to" becomes **Send again to another destination**. [D05](specs/D05-files-and-entry-detail.md), [D06](specs/D06-transfers-list.md) |
| D8 | Share modal | Reject | [Rejected patterns](#rejected-patterns). |
| D9 | Filter panel (size, type, modified, sharing) | Adapt | State, Category and search chips that are always visible. [D05](specs/D05-files-and-entry-detail.md), [D06](specs/D06-transfers-list.md) |
| D10 | Empty state: one sentence + one button | Adopt | Already a principle; made into a component. [01](01-design-system.md#empty-states) |
| D11 | Bin, Empty bin, Restore | Reject | The Source is never deleted (ADR 0005), and filebridge deletes nothing at the destination. |
| D12 | Mobile layouts | Adapt | A narrow desktop window, not a phone app. [02](02-information-architecture.md#window-sizes) |
| D13 | Story "know when my team changed files" | Adapt | **Send again** sends only what is new or changed. Scheduled sends stay an open decision. [D06](specs/D06-transfers-list.md) |
| D14 | Stories on retention and comments | Reject | No store, no other people. |
| D15 | Hypothesis: tags and shortcuts for forgotten names | Adapt | Job labels and Recent sources. [D06](specs/D06-transfers-list.md), [D02](specs/D02-source-and-composition.md) |
| D16 | Hypothesis: search and filter by name, type, date | Adopt | [D05](specs/D05-files-and-entry-detail.md), [D06](specs/D06-transfers-list.md) |
| D17 | Brand typeface and purple gradient | Reject (for now) | `system-ui` stays; a brand font waits for the final app name ([10](../10-risks-and-open-decisions.md#open-decisions)). |

## E — CloudVault: Cloud Storage Dashboard

**What it shows.** A dashboard:

- Top bar: search, "Create New", a bell, an avatar.
- Sidebar: Dashboard, All Files, Favorites, Shared, Recycle Bin, a Quick Access list of pinned
  folders, and a storage card ("650 GB of 1000 GB used", Upgrade Storage).
- **Storage Overview**: four coloured circles with % per type (Images, Documents, Videos,
  Others) and their sizes.
- **Quick Actions**: Upload Files, Create Folder, Share Link.
- **Folders**: six coloured tiles with file counts.
- **All Files**: Name, Size, Modified, Owner, ⋯.
- **Recently Modified**: cards with size, "2 hours ago", and the initials of the people involved.

| # | Feature | Verdict | Where / why |
|---|---|---|---|
| E1 | A dashboard as the first screen | Adopt | **Home**. [D01](specs/D01-home.md) |
| E2 | Storage overview by file type | Adapt | Two places: what a **transfer** contains (by Category), and how full a **destination** is. Shown as stacked bars, not circles. [D02](specs/D02-source-and-composition.md), [D03](specs/D03-destination-storage.md) |
| E3 | Quick actions | Adapt | New transfer, Add destination, Send again (last transfer). [D01](specs/D01-home.md) |
| E4 | Coloured folder tiles | Adapt | Destination tiles. [D01](specs/D01-home.md) |
| E5 | Recently Modified with relative times | Adapt | Recent transfers. [D01](specs/D01-home.md) |
| E6 | Favorites, Quick Access pinned folders | Adapt | Recent sources now; pinning **later**. [D02](specs/D02-source-and-composition.md) |
| E7 | Notification bell | Reject | OS notifications exist, and Home's "Needs you" section covers what a bell would list. |
| E8 | "Create New" as the top primary button | Adopt | "New transfer" is the first item in the sidebar, styled as a button. [02](02-information-architecture.md#sidebar) |
| E9 | Storage card with Upgrade | Adapt | The meter only. [D03](specs/D03-destination-storage.md) |
| E10 | Shared, Recycle Bin, Owner column | Reject | [Rejected patterns](#rejected-patterns). |

## F — Vault & Vine: File Manager App

**What it shows.** A mobile app built on a "Slate Blueprint" aesthetic: slate paper, graphite
text, and **one** cobalt accent kept for selection and primary actions. File types carry colours
(amber docs, teal media, coral archives) as small left-edge bars, rows are separated by hairline
rules instead of floating cards, and numbers use **tabular monospace**. The shot shows three of
its six screens; the description names all six:

1. **Home**: a stacked storage meter (Photos / Docs / Apps / Other with sizes), **source tiles**
   (Internal, SD Card, Cloud Drive, Downloads with item counts), and recent files.
2. **Browser**: breadcrumbs (INTERNAL › DOCUMENTS › WORK), an item count, a sort/grid switch,
   multi-select, and a bottom bar for the selection ("1 Selected": share, copy, delete).
3. **File detail**: a preview, the name, the kind, and a Parameters table (path, size in exact
   bytes, extension, created, modified, **Checksum (SHA1)**). Actions: Open File, Share, Rename.
4. **Search** with type filter chips.
5. **Transfers queue** with progress per file.
6. **Cleanup scan** that shows reclaimable space.

This is the closest shot to filebridge's purpose.

| # | Feature | Verdict | Where / why |
|---|---|---|---|
| F1 | One accent, reserved for selection and the primary action | Adopt | [01](01-design-system.md#colour) |
| F2 | File-type colour as a left-edge bar | Adopt | [01](01-design-system.md#file-categories), used in [D04](specs/D04-transfer-queue.md) and [D05](specs/D05-files-and-entry-detail.md) |
| F3 | Hairline rules instead of floating cards for rows | Adopt | [01](01-design-system.md#surfaces-and-rules) |
| F4 | Tabular monospace numerals | Adopt | [01](01-design-system.md#typography) |
| F5 | Stacked storage meter by type | Adopt | The `StorageMeter` component. [01](01-design-system.md#components), [D02](specs/D02-source-and-composition.md), [D03](specs/D03-destination-storage.md) |
| F6 | Source tiles (Internal, SD, Cloud, Downloads) | Adapt | Destination tiles on Home; Recent sources in the Source step. Mounted drives as quick sources: **later**. [D01](specs/D01-home.md), [D02](specs/D02-source-and-composition.md) |
| F7 | Recent files with type bars | Adapt | Recent transfers. [D01](specs/D01-home.md) |
| F8 | Floating + button, bottom tab bar | Reject | Desktop app. |
| F9 | Breadcrumbs | Adopt | The destination folder browser. [D11](specs/D11-destination-browser.md) |
| F10 | Multi-select with a bar for the selection | Adapt | Issues in the Analysis step. [D08](specs/D08-issue-selection.md) |
| F11 | Grid/list switch | Reject | One list view; see G1. |
| F12 | File detail with exact bytes and a checksum | Adopt | **Entry detail**: both paths, exact size, the Name rules applied, transfer and verify state, SHA-256. [D05](specs/D05-files-and-entry-detail.md) |
| F13 | Preview thumbnail in the detail | Later | A thumbnail of the **local** source image is cheap and safe; not needed to prove anything. [D05](specs/D05-files-and-entry-detail.md#out-of-scope) |
| F14 | Open File, Share, Rename | Adapt | **Show in file manager** (source) and Copy path. Share and Rename rejected. |
| F15 | Search with type chips | Adopt | [D05](specs/D05-files-and-entry-detail.md) |
| F16 | Transfers queue with progress per file | Adopt | [D04](specs/D04-transfer-queue.md) |
| F17 | Cleanup scan of reclaimable space | Adapt | **Shown, never freed** (ADR 0011). [D10](specs/D10-reclaimable-source.md) |

## G — slothUI: File Manager Dashboard ("My Cloud")

**What it shows.**

- The same shell as A, plus a List / Column / Card / Normal view switch.
- Folder tiles (Files 118 · 2.8 GB, Videos, Audio).
- An inline drop zone on the dashboard.
- A "My Files" table with a column of **user permission** avatars.
- A **My Storage donut** (85.1 GB) with a legend by category.
- **Recent Activity** ("Uploaded 25 Files", "Deleted 2 Files", "Shared 3 Files", with dates).
- A sidebar notice: "Space Almost Expired" with a bar and Dismiss / Go Pro.
- **Export Data** at the top right.

| # | Feature | Verdict | Where / why |
|---|---|---|---|
| G1 | View switcher | Reject | One well-made list beats four views; cards only for destinations. |
| G2 | Folder tiles with count and size | Adapt | Destination tiles. [D01](specs/D01-home.md) |
| G3 | A drop zone on the dashboard | Adopt | Dropping on Home starts a New transfer with those paths. [D01](specs/D01-home.md) |
| G4 | User permission avatars | Reject | [Rejected patterns](#rejected-patterns). |
| G5 | Storage donut with a legend | Adapt | A stacked bar: easier to compare and to read in a narrow window. [D02](specs/D02-source-and-composition.md), [D03](specs/D03-destination-storage.md) |
| G6 | Recent Activity feed | Adopt | **Activity**, from the persisted job log. [D07](specs/D07-job-log-and-activity.md) |
| G7 | "Space almost full" notice | Adapt | Low destination space notice. [D03](specs/D03-destination-storage.md) |
| G8 | Export Data | Adapt | Export a Job's **Report** (CSV / JSON) as proof of the migration. [D09](specs/D09-report-export.md) |
| G9 | Phosphor icons, WCAG 2.1 | Adopt | [01](01-design-system.md#icons), [01](01-design-system.md#accessibility) |

---

## Rejected patterns

Listed once so they don't come back without a new decision ([Q1](README.md#decisions-this-folder-rests-on)):

| Pattern | Seen in | Why not |
|---|---|---|
| Share links, invites, link permissions | A, D, E | filebridge owns no store to share from; sharing belongs to the destination's tool (Nextcloud). |
| Collaborators, groups, owners, permission avatars | A, D, E, G | One user, on their own machine. |
| Comments on files | D | No other people. |
| Bin, Empty bin, Restore, Recycle Bin | D, E | The Source is never deleted (ADR 0005), and filebridge never deletes at the destination. |
| Delete, Rename, Move, Version history on files | A, D, F | They act on a store filebridge doesn't own. Renaming happens only in transit, through Name rules. |
| Retention timers ("how long my file is kept") | D | Nothing expires; the destination keeps the files. |
| Subscription, upsell, Buy/Upgrade storage | A, D, E, G | Not a product with tiers. |
| Email recipients, privacy toggle | C | The destination is a Profile, not a person. |
| Global search across everything | A, E, G | The data is per Job; search lives in the two lists that have it. |
| View switchers, grid views | F, G | One list view done well. |
| Notification bell | E | OS notifications plus Home's "Needs you" section. |
| Marketing art, gradients, social proof | C, D | A long-running tool should look calm and factual. |
| Mobile app layouts, floating button, bottom tabs | D, F | Desktop app; a narrow window is covered in [02](02-information-architecture.md#window-sizes). |

## filebridge's own user stories

Following shot D's method, these are the stories the specs answer. "I" is the person migrating
their own data: the baseline user of [Q3](README.md#decisions-this-folder-rests-on).

| # | As the person migrating my data, I want to… | So that… | Spec |
|---|---|---|---|
| S1 | see at a glance whether anything needs me | a 31-hour send never sits stuck waiting for a click | D01 |
| S2 | see which files are being sent right now, and how fast each one goes | I can tell a slow disk from a stuck send | D04 |
| S3 | know what a transfer contains (photos, videos, documents…) before I send it | I can spot a wrong folder before sending 400 GB | D02 |
| S4 | know whether the destination has room, before and during the send | it doesn't fail at 95 % | D03 |
| S5 | find one file in a finished transfer and see where it went, under which name, and its checksum | I can prove it arrived intact | D05 |
| S6 | find an old transfer by name, destination or state | I can check what I sent last month | D06 |
| S7 | read what happened while the window was closed | I trust the background service | D07 |
| S8 | decide many name issues at once without accepting all of them | 2 000 issues don't take an afternoon | D08 |
| S9 | keep a file that proves the migration | I can archive it, or compare it later | D09 |
| S10 | know when it is safe to free space at the source | I don't delete something that never arrived | D10 |
| S11 | choose where on the destination the files land | I don't have to type paths on a server I can't see | D11 |
| S12 | send the same folder again later, sending only what changed | keeping a copy up to date is one click | D06 |
| S13 | add folders I send often without browsing for them | repeat sends are quick | D02 |

## Summary of verdicts

| Verdict | Count |
|---|---|
| Adopt | 28 |
| Adapt | 31 |
| Reject | 18 (one of them "for now": D17) |
| Later | 1 |

Every Adopt and Adapt item lands in one of: [01](01-design-system.md), [02](02-information-architecture.md),
or D01–D11.
