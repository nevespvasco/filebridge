# 01 — Design system

- Status: **Built** (see "As built")
- Phase: 1 (foundation; every spec in [specs/](specs/) builds on it)
- Inspired by: F (Vault & Vine), A and G (slothUI), B (upload modal), D (empty states)
- Layers touched: `apps/desktop` only (`src/style.css`, `src/components/`, `package.json`)

This doc fixes the visual language of `apps/desktop`. It **extends** what exists in
`apps/desktop/src/style.css` (tokens, `.card`, `.btn`, `.badge-*`, `.table`, `.guide`); it does
not replace it with a framework.

## Principles

1. **Calm and factual.** A send can run for 31 hours. The screen reports state: no
   gradients, illustrations or celebration. (Rejected: shot C's art, D's gradient.)
2. **One accent.** `--accent` marks only the **primary action** of a screen and the **current
   selection** (active nav item, selected profile, checked rows). Status uses its own colours, and
   file Categories use theirs. (From F.)
3. **Status is never colour alone.** Every state pairs a colour with an icon and a word
   (WCAG 1.4.1). This is already true of `JobStateBadge` and `StatusIcon`, and it stays the rule.
4. **Numbers line up.** Sizes, counts, speeds, dates and percentages use tabular figures, so
   columns don't jitter while progress updates. (From F.)
5. **Rows, not floating cards, in long lists.** Lists separate rows with hairline rules. Cards
   are for a few large things: profiles, summary tiles, dialogs. (From F.)
6. **Plain words first** (existing principle in [06](../06-interface.md#principles)): one sentence
   saying what to do now, a `HelpTip` for jargon, and a reason on every disabled button.

## Tokens

All tokens are CSS custom properties on `:root`, redefined for dark mode. The existing colour
tokens keep their names and values. New tokens are added next to them.

### Colour

Existing (unchanged): `--bg`, `--surface`, `--surface-2`, `--border`, `--border-strong`, `--text`,
`--text-2`, `--muted`, `--track`, `--accent`, `--accent-soft`, `--on-accent`, `--ok`, `--ok-soft`,
`--warn`, `--warn-strong`, `--warn-soft`, `--danger`, `--danger-soft`.

New:

| Token | Light | Dark | Use |
|---|---|---|---|
| `--rule` | `#e8ebef` | `#262b32` | Hairline between rows (softer than `--border`) |
| `--overlay` | `rgb(15 18 22 / 0.45)` | `rgb(0 0 0 / 0.6)` | Behind dialogs and drawers |
| `--focus` | `= --accent` | `= --accent` | Focus ring (an alias, so it can be tuned alone) |
| `--info` | `#2f6fb3` | `#7fb2ee` | Neutral notices that are not the accent (Activity "info" icon) |

The accent is never used for a status. "Running" uses `--accent` only because a running Job **is**
the current selection of the app's attention. That convention exists today in `JobStateBadge`
and is kept.

### File categories

A **Category** (see [GLOSSARY](../../GLOSSARY.md)) groups files by extension, so a transfer can
say what it contains. Each one has a colour, an icon and a label. The colour is **never** the only
signal: a Category always appears with its icon or its name.

| Category | Token | Light | Dark | Phosphor icon | en / pt label |
|---|---|---|---|---|---|
| `image` | `--cat-image` | `#0e9f8e` | `#35c9b5` | `FileImage` | Photos & images / Fotos e imagens |
| `video` | `--cat-video` | `#7656d6` | `#a48cff` | `FileVideo` | Videos / Vídeos |
| `audio` | `--cat-audio` | `#c2459a` | `#f27ac6` | `FileAudio` | Audio / Áudio |
| `document` | `--cat-document` | `#1d8ad0` | `#5cb6f2` | `FileText` | Documents / Documentos |
| `archive` | `--cat-archive` | `#c2620a` | `#f08a3c` | `FileZip` | Archives / Arquivos comprimidos |
| `code` | `--cat-code` | `#5f7f1f` | `#a3c75a` | `FileCode` | Code / Código |
| `other` | `--cat-other` | `#8a939e` | `#6f7883` | `File` | Other / Outros |

Rules:

- Category colours are used for **graphics only**: meter segments, left-edge bars, icons. Never
  for text. Each must reach **3:1** contrast against `--surface` in its theme (WCAG 1.4.11). Check
  this when tuning a value.
- None of them reuses a status hue: `--ok` green, `--warn` amber and `--danger` red stay
  unambiguous. Documents are sky blue, not amber (unlike shot F), because amber already means
  "warning" here.
- Directories are not a Category. They use `Folder` in `--text-2`.

Which extensions map to which Category is a domain table owned by `crates/model` (spec
[D02](specs/D02-source-and-composition.md#category-table)), so the CLI, the report and the UI agree.

### Typography

The font stays `system-ui` at 15px (as today). A brand font waits for the final app name.

| Token | Value | Use |
|---|---|---|
| `--fs-xs` | `0.75rem` | Badges, meter legends |
| `--fs-sm` | `0.85rem` | Hints, table headers, secondary lines |
| `--fs-base` | `1rem` (15px) | Body |
| `--fs-lg` | `1.2rem` | Section titles (`h2`) |
| `--fs-xl` | `1.5rem` | Page title (`h1`) |
| `--fs-stat` | `1.9rem` | The big number of a `StatTile` |
| `--font-mono` | `ui-monospace, "SF Mono", "Cascadia Mono", "DejaVu Sans Mono", monospace` | Hashes, paths, byte views |

New utility classes:

```css
.num   { font-variant-numeric: tabular-nums; }                 /* every size, count, %, speed, date */
.mono  { font-family: var(--font-mono); }                      /* exists; switch to the token */
.hash  { font-family: var(--font-mono); font-size: var(--fs-sm); letter-spacing: 0.02em; word-break: break-all; }
.path  { font-family: var(--font-mono); font-size: var(--fs-sm); overflow-wrap: anywhere; }
```

Where `.num` applies: `.table td`, `ProgressBar` labels, `JobProgress`, `StatTile`, the
`StorageMeter` legend, and the counts in badges. A table column of numbers is also right-aligned
(`.table td.n, .table th.n { text-align: right; }`).

### Spacing, radii, elevation

| Token | Value |
|---|---|
| `--sp-1` … `--sp-6` | `0.25rem`, `0.5rem`, `0.75rem`, `1rem`, `1.5rem`, `2rem` |
| `--radius-sm` | `6px` (chips, inputs) |
| `--radius-md` | `10px` (cards; the current `.card` value) |
| `--radius-lg` | `14px` (dialogs, drawers) |
| `--shadow-pop` | `0 8px 28px rgb(0 0 0 / 0.18)` (menus, dialogs, drawers only) |

Nothing that sits in the page flow gets a shadow. Only things floating above the page do.

### Surfaces and rules

- Page: `--bg`. Card, table and dialog: `--surface`. Hover and zebra striping: `--surface-2`.
- Rows in lists: `border-bottom: 1px solid var(--rule)`, with no card around each row.
- A list with a Category left-edge bar: `box-shadow: inset 3px 0 0 var(--cat-…)` on the first cell.

## Theme

Today dark mode follows `prefers-color-scheme` with no override. New:

- **Settings → Appearance**: a `SegmentedControl` with **System** (default) / **Light** / **Dark**.
- Stored like the other UI-only preferences (`stores/settings.ts`, localStorage, key
  `filebridge.theme`). It is a window preference, not an engine setting, so it doesn't go
  through the service.
- Applied as `document.documentElement.dataset.theme = "light" | "dark"`, or with the attribute
  removed for System. The CSS becomes:

```css
:root { /* light tokens */ }
@media (prefers-color-scheme: dark) { :root:not([data-theme="light"]) { /* dark tokens */ } }
:root[data-theme="dark"] { /* dark tokens */ }
```

- `color-scheme` follows the theme (`light`, `dark`, or `light dark`), so native controls and
  scrollbars match.
- The theme is applied before the first paint (a tiny inline script in `index.html` reads
  localStorage), so a dark-theme user never sees a white flash.

## Icons

Phosphor replaces emoji and Unicode glyphs (from A and G).

- Package: `@phosphor-icons/vue` (MIT), version pinned in `apps/desktop/package.json`. Import
  per icon (`import { PhFolder } from "@phosphor-icons/vue"`) so the bundle only carries what is
  used.
- One wrapper, `AppIcon.vue` (`name`, `size` = 16 | 20 | 24, `weight` = regular | fill | bold,
  `label?`). Without `label` it renders `aria-hidden="true"`; with `label` it renders
  `role="img"` and `aria-label`. Components never use Phosphor directly, so the icon set can
  change in one file.
- Sizes: 16px inline with text, 20px in nav and buttons, 24px in tiles and empty states (40px
  there).
- Weight: `regular` everywhere, `fill` for the active nav item and selected states, `bold` only
  in `StatusIcon`.

Replacement map for what exists today:

| Today | Where | Phosphor |
|---|---|---|
| `💡` | `StepSource` guide (since replaced by the guided tour, D02) | `PhLightbulb` |
| `👉` | `StepDestination`, `JobDetailView` guide | `PhHandPointing` |
| `✋` | `StepAnalysis` guide (decisions pending) | `PhHand` |
| `⚠️` | `StepAnalysis` guide (warning) | `PhWarning` |
| `✅` | `StepAnalysis` guide (all decided) | `PhCheckCircle` |
| `🔒` | `StepReview` safety; a source that can't be read (D02) | `PhLockSimple` |
| `☕` | `StepAfter` guide ("you can close the window") | `PhCoffee` |
| `📭` | `JobsView` empty state | `PhTray` |
| `🗂️` | `DestinationsView` empty state | `PhHardDrives` |
| `⇪` | `FileDropZone` | `PhArrowLineDown` (things are dropped *into* filebridge; no "upload" arrow, see Q6) |
| `📁` / `📄` | `FileDropZone` buttons | `PhFolderPlus` / `PhFilePlus` |
| `▸` / `▾` | `IssueGroup`, `NextcloudDetect` | `PhCaretRight` / `PhCaretDown` |
| `⏸` / `▶` | Job detail Pause / Resume | `PhPause` / `PhPlay` |
| `✕` | Remove a source | `PhX` |
| `✓ ! ✗` | `StatusIcon` | `PhCheck` / `PhWarningCircle` / `PhXCircle` (bold) |

New icons used by the specs:

| Meaning | Phosphor |
|---|---|
| Home | `PhHouse` |
| New transfer | `PhPaperPlaneTilt` |
| Transfers | `PhArrowsLeftRight` |
| Destinations | `PhHardDrives` |
| Settings | `PhGearSix` |
| Local destination | `PhHardDrive` |
| SFTP destination | `PhTerminalWindow` |
| Nextcloud destination | `PhCloud` |
| Folder | `PhFolder` |
| Verified | `PhSealCheck` |
| Checksum | `PhFingerprint` |
| Renamed in transit | `PhTextAa` |
| Skipped | `PhProhibit` |
| Retrying | `PhArrowClockwise` |
| Search | `PhMagnifyingGlass` |
| Row menu | `PhDotsThreeVertical` |
| Export report | `PhExport` |
| Show in file manager | `PhFolderOpen` |
| Copy | `PhCopy` |
| Reclaimable space | `PhBroom` |
| Send again | `PhArrowsClockwise` |
| Activity | `PhPulse` |
| Low space | `PhGauge` |
| Guide / help (`?`) | `PhQuestion` |
| Symbolic link | `PhLinkSimple` |
| Undo | `PhArrowUUpLeft` |

## Components

Existing components keep their roles: `BytesDiff`, `ConfirmDialog`,
`FileDropZone`, `HelpTip`, `IssueGroup`, `JobProgress`, `JobStateBadge`, `NextcloudDetect`,
`PreflightList`, `ProfileForm`, `ProgressBar`, `StatusIcon`. `HelpTip` also takes a default slot,
which then is the trigger instead of the "?" (the service status in the sidebar uses it: hover,
focus or tap the status itself). The new ones below are generic: each takes plain props and knows
nothing about the engine, so specs reuse them.

| Component | Props (sketch) | Behaviour | Used by |
|---|---|---|---|
| `AppIcon` | `name, size?, weight?, label?` | See [Icons](#icons) | everywhere |
| `StatTile` | `label, value, unit?, hint?, tone?` | Big tabular number, label under it; `tone` = muted/ok/warn/danger | D01, D02, D10 |
| `StorageMeter` | `segments: {key, label, bytes, color}[], total, mode: "usage" \| "composition"` | Stacked bar plus a legend with `.num` sizes and %. In `usage` mode the remainder is "free" in `--track`. Segments under 1 % get a minimum width of 2px. A `<title>` and a visually hidden table give the exact values to screen readers. | D02, D03, D01 |
| `CategoryBar` | `category` | The 3px left-edge bar on a row, with an `sr-only` category name | D04, D05 |
| `DataTable` | `columns: {key, label, sortable?, numeric?, width?}[], rows, sort, rowKey, selectable?, loading?` | Header sort buttons with `aria-sort`; keyboard row focus; emits `sort`, `open(row)`, `select(keys)`. Only the window around the visible rows is rendered when there are more than 200 (see below). | D05, D06 |
| `FilterChips` | `options: {value, label, count?, icon?}[], modelValue, multiple?` | A row of toggle chips (`aria-pressed`); a count badge on each; "All" clears | D05, D06 |
| `SearchField` | `modelValue, placeholder, debounce = 250` | Magnifier icon, clear button, `/` shortcut focuses it when its page is open | D05, D06, D08 |
| `RowMenu` | `items: {id, label, icon?, danger?, disabled?, reason?}[]` | ⋮ button opens a menu (`role="menu"`), arrow keys, Esc closes and returns focus; a disabled item shows its `reason` | D06, D05 |
| `Drawer` | `open, title, width = 420` | Side panel from the right with `--overlay`; focus trapped; Esc closes; deep-linkable through the route query | D05 |
| `SelectionBar` | `count, actions` | Sticky at the bottom of its list when `count > 0`: "N selected", actions, Clear. Announced through `aria-live="polite"`. | D08 |
| `Breadcrumbs` | `segments: {label, value}[]` | `nav` with `aria-label`; the last segment is `aria-current="page"`; long paths collapse the middle into "…" | D11 |
| `EmptyState` | `icon, title, text, action?` | See [Empty states](#empty-states) | everywhere |
| `SegmentedControl` | `options, modelValue` | `role="radiogroup"` | Settings (theme), D03 |
| `RelativeTime` | `at: number` | "3 min ago" / "há 3 min" through `Intl.RelativeTimeFormat`; the exact time in `title` and `<time datetime>` | D01, D06, D07 |
| `Notice` | `tone, icon, text, action?, dismissible?` | Inline bar, also used in the sidebar notice slot | D03, D10, [02](02-information-architecture.md#sidebar) |
| `Toast` | `text` | Short confirmation ("Path copied"), `aria-live="polite"`, 3 s | D05, D09 |
| `DropOverlay` | `show` | Full-window layer while files are dragged over it: dashed accent frame, "Drop to add". `aria-hidden`, `pointer-events: none` | D02 (`FileDropZone`) |
| `GuidedTour` | — (reads the tour store) | Mounted once in `App.vue`. Spotlight on a `data-tour` element (`box-shadow` with `--overlay`), a card with count, dots, Skip / Back / Next. `role="dialog"`, `#app` inert, focus trapped and restored, Esc skips, ← / → move. Steps are data in `src/lib/tour.ts` | D02 |
| `TourButton` | `tour` | The `?` next to a page title: plays its guide again. Just the circled-`?` icon, no ring of its own (the icon already is one) | D02 |
| `ClampText` | `text, lines = 2` | A long message cut after `lines` lines, with More / Less only when it is cut; re-measured when the width changes | Destinations, D03, `PreflightList` |

### Dialogs

Every dialog is a `ModalDialog` (`open, title, role?, width?, busy?`, slots for the body, the
title and the actions): a card over `--overlay`, named by its title (`aria-labelledby`), focus
moved in (to `[data-autofocus]`, else the first control) and kept in, Esc and the backdrop close
it unless `busy`, and focus goes back to the control that opened it. The focus handling is
`composables/useFocusTrap.ts`, shared with `Drawer`. `ConfirmDialog` is a `ModalDialog` whose
focus starts on the safe button; a flow that isn't a page's own action asks through the confirm
store (`useConfirm().ask(...)`), rendered once in `App.vue`.

### Forms

A field is a `FormField` (`label, help?, example?, hint?, required?`): the visible label is a
`<label for>` tied to the control, so a screen reader announces "Name", never the placeholder or
the help text; the `?` sits beside the label, and the hint under the control describes it
(`aria-describedby`). A saved secret is never shown again: "Saved in the system keyring" with
"Replace…".

Fields sit in a `.form-grid`: two columns while each gets at least 15rem, one below that, never
three, so rows stay even at every width. A field that needs the whole line (a folder with Browse…)
is `.wide`; a short value (a port) is `.narrow`, which shortens its input, not its column. A
`SegmentedControl` that doesn't fit puts each option on its own line instead of widening the page.

### Status

A state is icon + word + colour, never colour alone (`JobStateBadge`, `StatusIcon`). Given the
whole job, a finished one says what matters most: **Verified** (shield, green) when every file was
compared with no difference, or "N problems" (amber). Green is kept for "verified" and "no
differences"; the accent is for the one main action of a screen.

### Result card

`ResultCard` ends a transfer, in the wizard's last step and on top of its page: one big number in
`--fs-stat`, one sentence ("1,234 files copied and verified, one by one"), one line under it ("No
differences…"), the facts (size, names fixed, skipped, retries, when) and what to do next
(Export the report, See the problems, See the space to free). When files couldn't be sent, it
lists the first ten with their error and how many more ([ADR 0020](../adr/0020-give-up-on-a-file-after-three-retries.md)). Its numbers are the Files tab's own
facets, so the card and the list never disagree. Tones: green verified, accent copied without
verification, amber problems, red stopped, grey cancelled.

### Notices

`NoticeBox` (`tone, icon`) is the "what to do now" box (the `.guide` look): one or two sentences
and at most one action. `BulkDecision` is the one for names waiting for a decision: "Accept the N
suggestions" (every rule whose suggestions may be accepted without a look) and "Review one by
one"; garbled names are left out of the button, with the reason.

A check's result (a destination's test, its last space check) is a **status line**, not a red
paragraph: a `StatusIcon` and the message in body colour, cut with `ClampText`. Each thing is said
once: a failed space check isn't repeated when the test under it already failed, "doesn't report
its free space" isn't added under the error that explains it, and a `PreflightList` block whose
checks all pass is one line ("✓ Integration · can write") with its checks behind "N checks
passed". Technical information on shows every check.

### Toast

`useToast().show(text)` for the confirmation of a small action ("Copied"): bottom centre, above the
phone tab bar, `aria-live="polite"`, 2.5 s, one at a time (`ToastHost` in `App.vue`).

### Long lists

[06](../06-interface.md#principles) already says long lists are virtualised and paged from SQLite.
`DataTable` does both:

- **Paging** is keyset (`after` cursor) from the service, 200 rows per request. It loads the next
  page when the last 50 rows come into view.
- **Virtualisation**: only the rows in view plus an overscan of 20 are in the DOM. Use
  `@tanstack/vue-virtual` (MIT, headless: no styles, no components) rather than hand-rolled
  windowing. It is a utility, not a component library, so the "no component library" decision
  holds.
- Row height is fixed (40px; 48px with a second line), so scrolling is O(1).

## States

Every screen and component that loads data handles all four states the same way:

| State | Look |
|---|---|
| Loading (first time) | `SkeletonRows` in `--surface-2`, still when `prefers-reduced-motion` is set; never a page-wide spinner, and never the empty state before the first answer |
| Loading (refresh) | The old content stays; a thin indeterminate `ProgressBar` at the top of the card |
| Empty | `EmptyState` |
| Error | `LoadError`: the problem in plain words and a **Try again** action; the service's own message is folded under "Technical details". Never the raw message as the text, and never the empty state |
| Service disconnected | The app-wide red banner is the only place that says why. Lists keep their last content, greyed (`.stale`, `opacity: .6`); a list never read shows "Waiting for the copy engine" (`LoadError` does this by itself), with no search, filters or counts above it. Actions that need the engine are disabled with the reason "Copy engine not responding" (`useConnection().offline`) |

### Empty states

From D: one sentence, one button. `EmptyState` = a 40px icon in `--muted`, a title (one line), a
text (one sentence: what this list is and how it gets filled), and the button that fills it. For
example:

| Where | Title (en) | Text (en) | Action |
|---|---|---|---|
| Transfers | No transfers yet | Each time you send folders, the transfer shows up here with its progress. | New transfer |
| Destinations | No destinations yet | A destination is where your files go: a disk, a server or Nextcloud. | Add destination |
| Files of a transfer, filtered | Nothing matches | No file of this transfer matches these filters. | Clear filters |
| Activity | Nothing happened yet | What your transfers do, even with the window closed, appears here. | — |

## Motion

- Transitions only for meaning: progress bars ease width over 300 ms; drawers slide 200 ms; menus
  fade 120 ms.
- With `prefers-reduced-motion: reduce`, all of these become instant and indeterminate bars
  become a static striped fill.
- Nothing pulses or bounces. A running Job shows that it is alive through changing numbers, not
  through animation.

## Accessibility

Target: **WCAG 2.1 AA** (the level slothUI claims, and the right bar for an app whose user may
not be technical).

- Contrast: text 4.5:1, large text and graphics 3:1, in both themes. Check the new tokens with any
  contrast checker before merging.
- Every interactive element is reachable and usable from the keyboard. The focus ring is
  `2px solid var(--focus)` with `outline-offset: 2px`, and is never removed.
- Live regions: `JobProgress` announces state changes (not every byte) through one
  `aria-live="polite"` region, throttled to once every 10 s. Errors use `role="alert"`.
- Tables use real `<table>` semantics, `scope="col"`, and `aria-sort` on sortable headers.
- Targets are at least 32×32px (24px for the icon in dense table rows, with spacing around it).
- Language: `<html lang>` follows the chosen locale (`pt-PT` / `en`).

## i18n rules

- Every string exists in `src/i18n/en.json` and `src/i18n/pt.json` with the same keys and the same
  number of plural forms (`apps/desktop/src-tauri/tests/i18n.rs`). The same test fails if a string
  quotes a number of wizard steps other than `SEND_STEPS`.
- Numbers, sizes and dates go through `Intl` with the locale (`pt-PT`, `en-GB` as today).
- Byte sizes use 1000-based units (kB, MB, GB, TB), as rclone and most file managers show them,
  with the exact byte count in `title` (and visible in Entry detail).
- No string is built by joining translated fragments; use named parameters
  (`"{files} files to {destination}"`).
- Category labels live under `category.*` (see [the category table](#file-categories)).

## As built

- Built as specified: tokens (colour, Category, type, spacing, radii, shadow), the theme setting
  with `public/theme-boot.js`, `AppIcon` over `@phosphor-icons/vue`, tabular numbers, and the
  components `StorageMeter`, `FilterChips`, `SearchField`, `RowMenu`, `Drawer`, `SelectionBar`,
  `Breadcrumbs`, `RelativeTime` and `SegmentedControl`; later `DropOverlay`, `GuidedTour` and
  `TourButton` (D02's Source step redesign). `FileDropZone` gained a `compact` one-line form, and
  its listen-while-on-screen logic moved to `composables/useFileDrop.ts`.
- Added after the UX audit of 2026-10-23: `ModalDialog` (all seven dialogs), `FormField`,
  `EmptyState`, `NoticeBox`, `Toast` (`ToastHost`), `SkeletonRows`, `ResultCard`, `BulkDecision`;
  `JobStateBadge` gained an icon per state. Font sizes and radii are tokens everywhere
  (`--radius-xs` and `--radius-pill` added). The "Show technical information" setting
  (`DetailsPanel`, check codes, "Copy transfer id") was removed on 2026-10-24.
- Done differently: there is no generic `DataTable`. The one list that needs virtualisation (the
  Files tab) has its own `role="grid"` built on `@tanstack/vue-virtual`, and the other tables are
  short, offset-paged `<table>`s. `CategoryBar` and `StatTile` are inline styles, not components:
  the big numbers use `--fs-stat` where they appear.
- The sidebar notice slot holds low destination space (D03) and reclaimable space (D10); the
  "service not installed as a user unit" notice isn't built.

## Acceptance criteria

1. No emoji or Unicode glyph is used as an icon anywhere in `apps/desktop/src`. A grep for the
   characters in the [replacement map](#icons) finds none.
2. Settings has System / Light / Dark. The choice survives a restart, and a reload in Dark shows no
   light flash.
3. Every number in `JobsView`, `JobProgress` and the counts cards uses tabular figures (no width
   jitter while progress updates).
4. All new tokens have light and dark values, and pass 4.5:1 (text) or 3:1 (graphics) against
   `--surface`.
5. `StorageMeter`, `DataTable`, `FilterChips`, `RowMenu`, `Drawer`, `SelectionBar`,
   `Breadcrumbs`, `EmptyState` and `SegmentedControl` each work by keyboard alone.
6. The `en` and `pt` locale files have the same key set (unit test).
