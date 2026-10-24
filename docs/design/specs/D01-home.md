# D01 — Home

- ID: D01 · Status: **Built** · Phase: 2
- Inspired by: E1, E3, E4, E5 (CloudVault dashboard), G2, G3, G6 (slothUI My Cloud), F6, F7
  (Vault & Vine home), D3 (UpLink Recent)
- Layers touched: `apps/desktop` only. Reads the operations of [D03](D03-destination-storage.md),
  [D06](D06-transfers-list.md), [D07](D07-job-log-and-activity.md) and [D10](D10-reclaimable-source.md);
  adds none of its own.
- Depends on: [01](../01-design-system.md), [02](../02-information-architecture.md). Each section
  works with whatever of D03/D06/D07/D10 has shipped and is hidden until its data source exists.

## Problem

The app opens on step 1 of the wizard (`/` → `/new`). Someone who comes back to a send that ran
overnight has to open Transfers, find the job and read its state. Nothing in one place answers
"does anything need me, and is everything fine?" (story S1). The jobs store already counts
running jobs for the sidebar badge, but nothing else is summarised.

## User stories

- S1: see at a glance whether anything needs me.
- As the person migrating, I want to start a new transfer by dropping folders on the first
  screen I see (G3).

## Design

The drop zone spans the full width under the Headline. Below it, two columns at ≥ 1100px, one
column below that (order top to bottom = order in the single column). Destinations and their free
space live on the Destinations page ([D03](D03-destination-storage.md)), not on Home.

```
┌ Home ─────────────────────────────────────────────── [ New transfer ] ┐
│ Everything is running normally.          (one sentence, see Headline)  │
│ ┌ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ┐ │
│              Drop files or folders here to start a transfer           │
│ └ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ┘ │
├────────────────────────────────────────────┬───────────────────────────┤
│ NEEDS YOU (only if any)                    │ RECLAIMABLE (only if any) │
│ ┌────────────────────────────────────────┐ │ 🧹 120 GB verified at     │
│ │ ✋ Photos 2019 → Nextcloud home         │ │   Nextcloud home [Review] │
│ │   12 names need a decision  [Decide]   │ │                           │
│ ├────────────────────────────────────────┤ │ ACTIVITY                  │
│ │ ✗ Work → Backup disk: destination      │ │ ✓ Sent 25 412 files to    │
│ │   folder missing          [Open]       │ │   Backup disk · 2 h ago   │
│ └────────────────────────────────────────┘ │ ↻ Retrying Nextcloud home │
│                                            │   (attempt 3) · 3 h ago   │
│ NOW SENDING                                │ + Created "Work" · 1 d    │
│ ┌────────────────────────────────────────┐ │ [See all in Transfers]    │
│ │ Photos 2019 → Nextcloud home    61 %   │ │                           │
│ │ ███████████████░░░░░░░░ 240/393 GB     │ │                           │
│ │ 48 MB/s · about 52 min left            │ │                           │
│ │ IMG_2041.jpg, IMG_2042.jpg +6 more     │ │                           │
│ └────────────────────────────────────────┘ │                           │
│                                            │                           │
│ RECENT TRANSFERS                           │                           │
│ Photos 2018   Nextcloud home  ✓  41 GB 2d  │                           │
│ Work          Backup disk     ✓  3 GB  3d  │                           │
│ Scans         Nextcloud home  ✗  —     5d  │                           │
│ [All transfers]                            │                           │
└────────────────────────────────────────────┴───────────────────────────┘
```
(The glyphs in the wireframe stand for Phosphor icons; see [01](../01-design-system.md#icons).)

### Sections

| Section | Shows | Data | Hidden when |
|---|---|---|---|
| **Headline** | One sentence from the worst current condition: "2 transfers need you", "1 transfer is sending", "Everything is up to date", "No transfers yet" | Derived from the sections below | never |
| **Needs you** | One row per job in the `needs_you` group ([D06](D06-transfers-list.md#job-groups)): the label (or the first source's name), the destination, the reason in plain words, one action | `list_jobs({ groups: [NeedsYou] }, 10, 0)` | no such job |
| **Now sending** | A compact `JobProgress` per running job (at most 3, then "+N more"), plus the names of in-flight files from [D04](D04-transfer-queue.md). A job in `Waiting` (held by the app-wide send window) shows "Waits for the send window: opens at 22:00" from its `SendWindowWait` event, with no speed or ETA | `list_jobs({ groups: [Active] })` + live `TransferProgress` | nothing running |
| **Drop zone** | A dashed area: "Drop folders here to start a transfer". Clicking it opens the folder picker | `FileDropZone` (exists) | never |
| **Recent transfers** | The last 5 finished jobs: label, destination, state icon, size, `RelativeTime` | `list_jobs({ groups: [Finished, Stopped] }, 5, 0)` | no finished job |
| **Activity** | The last 8 items from [D07](D07-job-log-and-activity.md) | `activity(None, 8)` | D07 not shipped |
| **Reclaimable** | A total and one row per eligible job ([D10](D10-reclaimable-source.md)) | `list_jobs({ reclaimable: true })` | none eligible |

The reasons in **Needs you** come from the job state and the counts. No free text from the
engine is shown:

| State | Reason (en) | Action |
|---|---|---|
| `AwaitingApproval` | "{n} names need a decision" | **Decide** → `/new` resuming that draft's step 3 (as "Continue in wizard" does today) |
| `Failed` | The job's `error`, first sentence, through the i18n map of known errors; otherwise "Stopped by an error" | **Open** → `/jobs/:id` |
| `Retrying` for > 1 h | "Retrying for {duration}: {error}" | **Open** |
| `CompletedWithWarnings` (not yet opened since it finished) | "Finished with {n} warnings" | **Review** → `/jobs/:id` |

"Not yet opened since it finished" is a window-side read marker (`stores/jobs`, localStorage
`filebridge.seen.<jobId>`). It is per window and not engine state.

### Quick actions

There are no separate quick-action buttons (E3): New transfer is the page's primary button,
**Add destination** is on the Destinations page, and **Send again** is on each Recent row's
`RowMenu` ([D06](D06-transfers-list.md#send-again)).

## Behaviour

- **Drop on Home**: dropping paths navigates to `/new` with the paths handed to the `wizard`
  store (not through the URL; see [02](../02-information-architecture.md#navigation-map)). The
  wizard opens on step 1 with them listed.
- **Live**: Now sending and Needs you update from the existing `service-event` subscription
  (`stores/jobs`). The lists are re-queried on `JobStateChanged`, at most once a second.
- **First run** (no profiles and no jobs) **[distribution]**: Home shows a 3-step onboarding card
  instead of the empty sections: "1 Add a destination · 2 Choose folders · 3 Send". Each step
  turns into a check when done. Hidden for good once a job completes.
- **Routing**: `/` redirects to `/home` once this ships ([02](../02-information-architecture.md#navigation-map)).

### As built

- `stores/home.ts` runs four bounded `list_jobs` queries (needs you, active, finished or
  cancelled, and a count) and re-runs them at most once a second when a job changes state.
- "Not yet opened since it finished" is kept per window in `localStorage` (`filebridge.seen`),
  marked when Transfer detail opens.
- Home's drop zone and the wizard's share one component, which now listens to window drops only
  while it is on screen, so the kept-alive wizard no longer takes drops meant for other pages.
- One column below 1100px.

## States

| State | Behaviour |
|---|---|
| Loading | Skeleton tiles per section; the Headline says "Loading…" |
| Service disconnected | Sections keep their last content, greyed; the Headline says "Not connected to the service"; the drop zone still works (the wizard handles the disconnection) |
| One section's call fails | That section shows an inline `Notice` with Try again; the others render normally |

## Data and contracts

None of its own. Everything is listed in the specs it reads:

- `list_jobs(filter, limit, offset)` with job groups: [D06](D06-transfers-list.md#data-and-contracts)
- `profile_storage(id, refresh)`: [D03](D03-destination-storage.md#data-and-contracts)
- `activity(before_id, limit)`: [D07](D07-job-log-and-activity.md#data-and-contracts)
- `JobFilter.reclaimable`: [D10](D10-reclaimable-source.md#data-and-contracts)

New window code: `views/HomeView.vue`, plus `stores/home.ts` (it composes the calls above and
throttles the re-query).

## i18n

Namespace `home.*`. Examples:

| Key | en | pt |
|---|---|---|
| `home.title` | Home | Início |
| `home.headline.needsYou` | {n} transfer needs you \| {n} transfers need you | {n} transferência precisa de si \| {n} transferências precisam de si |
| `home.headline.sending` | {n} transfer is sending \| {n} transfers are sending | {n} transferência em curso \| {n} transferências em curso |
| `home.headline.allGood` | Everything is up to date | Está tudo em dia |
| `home.needsYou` | Needs you | Precisa de si |
| `home.nowSending` | Now sending | A enviar agora |
| `home.recent` | Recent transfers | Transferências recentes |
| `home.drop` | Drop folders here to start a transfer | Largue pastas aqui para começar uma transferência |
| `home.reason.decide` | {n} names need a decision | {n} nomes precisam de uma decisão |

## Accessibility

- Each section is a `<section>` with an `<h2>`, so screen-reader users can jump between them.
- The Headline is the page's `aria-live="polite"` region, announced only when its sentence changes.
- The drop zone has a real button inside ("Choose folders"), so it works without drag and drop.

## Acceptance criteria

1. With a job in `AwaitingApproval`, Home shows it under Needs you with "N names need a decision",
   and **Decide** opens the wizard on that job's step 3.
2. With nothing running and nothing to decide, the Headline reads "Everything is up to date"
   and Needs you / Now sending are not rendered.
3. Dropping two folders on Home opens `/new` with both listed in step 1. Neither path appears in
   the URL.
4. Home renders in under 300 ms with 10 000 jobs in the database. It never lists all jobs: every
   section is a bounded query.
5. Every section degrades on its own: if `activity` fails, the rest of Home still renders.

## Out of scope

- A notification bell or inbox (E7, rejected).
- Pinned folders on Home (E6): later, with [D02](D02-source-and-composition.md#out-of-scope).
- Charts of transfer history (bytes per day). Nothing asks for them yet.

## Open questions

- Should Home also show an **Activity** entry for app-level events (service started, updated)?
  The proposal is no: only job milestones and profile changes ([D07](D07-job-log-and-activity.md)).
