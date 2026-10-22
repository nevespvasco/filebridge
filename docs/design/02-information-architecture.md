# 02 — Information architecture

- Status: Proposed
- Phase: 1 (navigation and page anatomy); Home itself is [D01](specs/D01-home.md), Phase 2
- Inspired by: A and G (sidebar, counters, notice slot), E (dashboard first, "Create New"), F (one
  accent for the current place)
- Layers touched: `apps/desktop` (`App.vue`, `router.ts`, views)

## Today

- `App.vue`: a 200px sticky sidebar (brand, New transfer, Transfers with a badge of running jobs,
  Destinations, Settings, the service status dot) and a main column up to 1100px wide.
- `router.ts`: `/` redirects to `/new`. Routes are `/new`, `/jobs`, `/jobs/:id`, `/destinations`
  and `/settings`, with hash history.
- Window: 1100×760 by default, minimum 360×480 (`src-tauri/tauri.conf.json`). Below 760px the
  sidebar becomes a top bar (see [Window sizes](#window-sizes)).

## Navigation map

```
Sidebar
├── [ New transfer ]          primary button style (E8); Ctrl+N
├── Home                      /home               (D01)
├── Transfers   (3) •         /jobs               (D06): badge = running, dot = needs you
│     └── Transfer detail     /jobs/:id?tab=overview|files|log&entry=<idx>
├── Destinations              /destinations       (D03, D11)
├── Settings                  /settings
├── ───────────
├── Notice slot               (A10/G7): at most one notice
└── Service status            (exists)
```

| Route | View | Default | Notes |
|---|---|---|---|
| `/` | redirect | → `/home` when Home ships, `/new` until then | First run with no profiles goes to `/home`, which shows onboarding (D01) |
| `/home` | `HomeView` | | D01 |
| `/new` | `WizardView` | | Kept alive (as today). Accepts `?sources=` from a drop on Home and `?again=<jobId>` from Send again (D06) |
| `/jobs` | `JobsView` | | Query keeps filters: `?q=&group=&profile=&sort=` so Back restores them |
| `/jobs/:id` | `JobDetailView` | `tab=overview` | Tabs: Overview, Files (D05), Log (D07). `entry=<idx>` opens the Entry detail drawer |
| `/destinations` | `DestinationsView` | | `?browse=<profileId>` opens the folder browser (D11) |
| `/settings` | `SettingsView` | | Adds Appearance ([01](01-design-system.md#theme)) |

Route query strings carry only ids, filters and indices. Never paths or anything personal: the
privacy rule against putting personal data in URLs applies to hash routes too, since they can end
up in logs and crash reports.

## Sidebar

- **New transfer** is the first item, styled as a full-width `btn-primary` (from E's "Create
  New"). It is the one place the accent appears in the sidebar, apart from the active item.
- **Active item**: `--accent-soft` background, `--accent` text, `fill`-weight icon (as today, plus
  the icon).
- **Transfers badge**: the number of running jobs (exists). New: a 6px `--warn` **dot** when any
  job needs the user: `AwaitingApproval`, `Failed`, or `Retrying` for more than 1 hour. *As
  built:* "more than 1 hour" is read as attempt ≥ 8, where the backoff (30 s … 15 min) has added
  up to about an hour (`needsYou` in `stores/jobs.ts`). The dot
  has `aria-label="Needs your attention"` and a tooltip listing how many.
- **Notice slot** (from A10 and G7, without the upsell): a single `Notice` above the service status,
  highest priority first:
  1. The service is not installed as a user unit (Linux): "Sends stop when you log out —
     Install". Dismissible, then remembered.
  2. A destination is low on space (D03): "Nextcloud home has 4 GB free" → opens Destinations.
     Dismissible until the next test.
  3. Space can be reclaimed (D10): "120 GB verified and reclaimable" → opens the transfer.
     Dismissible per job.
- **Service status** stays at the bottom (exists). Hovering, focusing or tapping it explains what
  it means; it has no separate `?`.

Width stays 200px, and the labels never truncate in en or pt. "Transferências" is the longest
label, and it fits at 15px. A window shorter than the sidebar scrolls it. Below 760px the sidebar
becomes a top bar and a bottom tab bar ([Window sizes](#window-sizes)).

## Page anatomy

Every page uses the same frame:

```
┌──────────────────────────────────────────────────────────────┐
│ ‹ Transfers                                   (back link, A3) │  only on detail pages
│ Page title                         [secondary] [Primary]      │  .page-head
│ One plain sentence: what this page is for / what to do now.   │  .lead-text
├──────────────────────────────────────────────────────────────┤
│ Notice (optional, page-level)                                 │
│ Toolbar: SearchField · FilterChips · sort         (lists only)│
│ Content                                                       │
└──────────────────────────────────────────────────────────────┘
```

- At most **one** primary button per page (rule from [01](01-design-system.md#principles)).
- Detail pages put tabs under the head (`role="tablist"`). Tab changes update `?tab=` without
  adding history entries (`router.replace`).
- The main column keeps `max-width: 1100px` (1280px from a 1600px window) and is centred in the
  space beside the sidebar, so a wide monitor gets even margins. Tables can use the full width;
  text blocks stay ≤ 70ch.

## Transfer detail: tabs

Today `JobDetailView` is one long page. It becomes three tabs:

| Tab | Content | Spec |
|---|---|---|
| **Overview** | Actions, `JobProgress`, the in-flight queue while sending (D04), the "What" and "Counts" cards, the reclaimable panel when eligible (D10), Export report (D09) | existing + D04, D09, D10 |
| **Files** | The Manifest as a table with filters, and the Entry detail drawer | D05 |
| **Log** | The persisted job log merged with live events | D07 |

The badge next to **Files** shows problems (`failed + mismatch + missing`) when there are any.

## Keyboard

| Keys | Action | Where |
|---|---|---|
| `Ctrl/⌘ + N` | New transfer | everywhere |
| `Ctrl/⌘ + 1…5` | Home, New transfer, Transfers, Destinations, Settings (`1` opens `/`, which is New transfer until Home ships) | everywhere |
| `/` | Focus the page's search | lists |
| `Esc` | Close the drawer, menu or dialog; clear an empty search | everywhere |
| `J` / `K` | Next / previous row | `DataTable` |
| `Enter` | Open the focused row | `DataTable` |
| `Space` | Toggle selection | selectable lists (D08) |
| `Shift + click` | Select a range | selectable lists (D08) |

Shortcuts are listed in Settings → Keyboard. They never fire while focus is in a text input,
except `Esc`.

## Window sizes

Every screen works from 360px wide (the window's minimum) up to any monitor, with nothing scrolling
sideways except code in "Technical details".

| Width | Layout |
|---|---|
| ≥ 1600px | Sidebar + main; the main column grows to 1280px, centred |
| 1100–1599px | Sidebar + main (1100px, centred); Home in two columns (D01) |
| 760–1099px | Sidebar + main; Home in one column; lists drop columns or become cards by the width they get (below) |
| < 760px | The sidebar splits in two, **both always on screen**. A **top bar** (sticky): brand, service status and New transfer on one row, the notice slot as a strip under it. The status shows, while running, only its green dot; otherwise a short badge, "Offline" or "Connecting…" (`conn.short`), since the banner under it says what happened. The full sentence stays for screen readers. A **bottom tab bar** (fixed, `--nav-bottom` = 3.6rem tall): Home, Transfers, Destinations and Settings as even tabs, icon over label, the active one marked on top, with the Transfers badge and dot. Whatever sticks to the bottom of a page (the Source step's footer, the selection bar) stands on the tab bar, and scrolling into view keeps clear of both bars |
| < 560px | Footers put their hint on its own line above Back / Next; page heads wrap their actions under the title; the Drawer is full width |
| < 400px | New transfer in the top bar keeps its icon and accessible name, not its text |

Lists and rows that sit in columns of different widths respond to **their own width**
(CSS container queries), not the window's, so they behave the same in Home's side column, a
Drawer or a narrow window:

| Component | Narrow form |
|---|---|
| Transfers table (D06) | Hides Size under 860px; under 700px each row is a card: name, state and ⋮ on top, the bar when running, then destination · files · size · when. The rows keep explicit table roles, and the column headers stay for screen readers |
| Home, Recent transfers | Under 520px: name and state on top, destination · size · when under them |
| Files tab (D05) | Hides Folder under 760px; under 520px each file takes two lines (name and status, folder and size) in the same row height |
| Sending now (D04) | Under 560px each file takes two lines: name and size, then bar, % and speed |
| Name decisions (`IssueGroup`) | Buttons go under the names (and the "choose" column header goes) under 680px; the name at the destination gets its own line under 480px. The byte diff wraps between bytes, "after" under "before" |
| Wizard stepper | Under 700px every step keeps its number and only the current one its label |
| Source list | Under 460px the parent folder goes under the name |

A narrow window is the only "mobile" layout. No phone app is planned (D12 in the
[review](00-reference-review.md#d--uplink-file-transfer-service-case-study)).

## Acceptance criteria

1. `/` lands on Home (built with D01).
2. "New transfer" is the first sidebar item, styled as the primary button, with `Ctrl/⌘+N`
   working from any page.
3. The Transfers item shows the running count and, separately, the attention dot, each with an
   accessible name.
4. Transfer detail has Overview / Files / Log tabs, `?tab=` survives a reload, and Back from a
   tab returns to Transfers (tabs don't add history entries).
5. The notice slot shows at most one notice, in the priority order above. A dismissed notice stays
   dismissed until its condition is new.
