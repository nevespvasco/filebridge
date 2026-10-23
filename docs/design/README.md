# Design — reference review, design system and feature specs

This folder turns a review of seven public file-transfer and file-manager designs (Dribbble shots,
listed in [00 — Reference review](00-reference-review.md)) into concrete proposals for filebridge:
what to adopt, what to adapt, what to reject, and how each accepted idea is built through the
layers in [CLAUDE.md](../../CLAUDE.md) and [08 — Repository structure](../08-repository-structure.md).

Everything here is **Proposed** until it is ticked in [09 — Roadmap](../09-roadmap.md). The specs
do not change any rule in the numbered docs or the ADRs; where one needs a decision, it links the
ADR that records it.

## Reading order

1. [00 — Reference review](00-reference-review.md): every shot, every feature it shows, and the
   verdict for each one (adopt / adapt / reject), plus filebridge's own user stories.
2. [01 — Design system](01-design-system.md): tokens, type, icons, file categories, components,
   states, accessibility.
3. [02 — Information architecture](02-information-architecture.md): navigation, routes, page
   anatomy, keyboard, window sizes.
4. Feature specs, in [specs/](specs/):

| ID | Spec | Phase | Layers touched |
|---|---|---|---|
| D01 | [Home](specs/D01-home.md) — **Built** | 2 | UI only (reads D03, D06, D07) |
| D02 | [Source step and transfer composition](specs/D02-source-and-composition.md) — **Built** | 2 | UI, `model`, `store` (migration), `ipc` |
| D03 | [Destination storage](specs/D03-destination-storage.md) — **Built** (two parts later) | 2 | UI, `transport-api` (additive), `transport-rclone`, `store` (migration), `ipc` |
| D04 | [Transfer queue](specs/D04-transfer-queue.md) — **Built** | 1 | UI, `transport-api` (additive), `transport-rclone`, `model` event, `core` |
| D05 | [Files of a transfer and Entry detail](specs/D05-files-and-entry-detail.md) — **Built** | 2 | UI, `model`, `store`, `core` (persist hash), `ipc` |
| D06 | [Transfers list](specs/D06-transfers-list.md) — **Built** | 2 | UI, `model` rules, `store` (migration), `ipc` |
| D07 | [Job log and Activity](specs/D07-job-log-and-activity.md) — **Built** | 1 (log) / 2 (Activity) | UI, `core`, `store`, `ipc` |
| D08 | [Issue selection](specs/D08-issue-selection.md) — **Built** | 2 | UI, `store`, `ipc` |
| D09 | [Report export](specs/D09-report-export.md) — **Built** | 2 | UI, `core`, `store`, `ipc` |
| D10 | [Reclaimable source](specs/D10-reclaimable-source.md) — **Built** | 2 | UI, `core`, `store` (migration), `ipc` — [ADR 0011](../adr/0011-reclaimable-source-is-shown-never-freed.md) |
| D11 | [Destination folder browser](specs/D11-destination-browser.md) — **Built** | 2 | UI, `transport-api` (new method), `transport-rclone`, `transport-fake`, `core`, `ipc` |

The design-system foundation ([01](01-design-system.md)) and the navigation
([02](02-information-architecture.md)) go first, in Phase 1, because every spec builds on them.

**Built** (branch `design/implementation`): everything in this folder — the design system and
navigation, and D01–D11. Each spec ends with an "As built" note where the code differs from it,
and lists what is left for later.

## Decisions this folder rests on

These were put as questions while reviewing the shots. The recommended answers were taken as the
working position so the docs could be written; each one can be reopened.

| # | Decision | Consequence |
|---|---|---|
| Q1 | filebridge stays a **send-and-prove** tool, not a file manager or cloud drive. Every idea is judged by "does it help pick, send, prove or recover a Job?" | Sharing links, collaborators, permissions, groups, comments, Bin, upsell and retention timers are rejected, with the reason recorded in [00](00-reference-review.md#rejected-patterns). |
| Q2 | One design folder; each candidate has a verdict, the layers it touches, a phase and the shot it came from. `06` and `09` change only to point here and to list the proposed items. | This folder. |
| Q3 | The baseline user is **personal use** (one person, their own servers, long migrations). Items that matter only when strangers use the app are tagged **[distribution]**. The open decision in [10](../10-risks-and-open-decisions.md#open-decisions) stays open. | Onboarding polish is tagged, not dropped. |
| Q4 | Visual language: **Phosphor** icons in place of emoji, **tabular numerals** and monospace for hashes and paths, a **colour per file Category**, a **light / dark / system** toggle, still hand-written CSS tokens and **no component library**. | [01 — Design system](01-design-system.md). |
| Q5 | "Free the source" means filebridge **shows** what is reclaimable and opens the folder in the OS file manager; it **never deletes** anything at the source. | [ADR 0011](../adr/0011-reclaimable-source-is-shown-never-freed.md), [D10](specs/D10-reclaimable-source.md). |
| Q6 | Code and docs say **Job**; the UI says **Transfer** (pt: *Transferência*); nobody says "upload". | [GLOSSARY](../../GLOSSARY.md). |

## How a spec is written

Every spec in [specs/](specs/) has the same sections, so they can be built one at a time:

- **Header**: ID, status, phase, the shots it comes from, layers touched, dependencies.
- **Problem**: what the user can't do or can't see today, with the code that shows it.
- **User stories**: filebridge's own, not the shot's.
- **Design**: a wireframe (ASCII), the parts of the screen and what each shows.
- **Behaviour**: interactions, live updates, pagination, edge cases.
- **States**: loading, empty, error, service disconnected.
- **Data and contracts**: the Rust types (sketch), the operations added to the catalogue in
  `crates/ipc/src/operations.rs`, events, store functions and migrations. Each says which crate it
  lives in, so the layering rules hold.
- **i18n**: the key namespace and example strings in en and pt.
- **Accessibility**.
- **Acceptance criteria**: testable statements.
- **Out of scope** and **open questions**.

## Cross-spec plan

Several specs touch the same files. These tables fix the order so they don't collide.

### Schema migrations (`crates/store/src/schema.rs`)

Migration numbers are assigned in landing order. The labels below are what the specs use. The
send-window work in progress (app-wide bandwidth limit and send window, `JobState::Waiting`) may
land its own migration first; these labels don't assume any number.

| Label | Change | Spec |
|---|---|---|
| M-category | `manifest_entries.category TEXT NOT NULL DEFAULT 'other'`; index `(job_id, category)` | D02 |
| M-profile-test | `profiles.last_test`, `profiles.last_test_at`, `profiles.last_good_test` | D03 |
| M-entry-index | indexes `manifest_entries(job_id, verify_state)`, `(job_id, size, idx)`, `issues(job_id, entry_idx)`, `events(kind, id)` | D05, D07, D08 |
| M-job-label | `jobs.label`, `jobs.again_of`, `jobs.reclaim_dismissed_at`, `jobs.reclaim_check`; `issues.carried_over`; indexes `jobs(state, created_at)`, `jobs(profile_id, created_at)` | D06, D10 |
| M-last-target (9) | `profiles.last_target_dir` | D11 |

`manifest_entries.mtime` and `hash`/`hash_algo` already exist. D05 starts writing the hash, and
D10 starts writing mtime; neither needs a migration. No migration edits a shipped one (rule in
`schema.rs`).

### Operation catalogue (`crates/ipc/src/operations.rs`)

`PROTOCOL_VERSION` is bumped once per release that changes the catalogue, not once per operation.

| Operation | New / changed | Spec |
|---|---|---|
| `job_categories(job_id) -> Vec<CategoryCount>` | new | D02 |
| `recent_sources(limit) -> Vec<RecentSource>` | new | D02 |
| `inspect_sources(paths) -> Vec<SourceInfo>` | new (protocol 15); `create_job` now refuses nested sources | D02 |
| `profile_storage(id, refresh) -> StorageView` | new | D03 |
| `entries(job_id, filter, after, limit) -> EntryPage` | new | D05 |
| `entry_facets(job_id) -> EntryFacets` | new | D05 |
| `entry(job_id, idx) -> EntryDetail` | new | D05 |
| `list_jobs(filter, limit, offset) -> JobPage` | **changed** (was `list_jobs(limit, offset) -> Vec<JobSummary>`) | D06 |
| `set_label(job_id, label)` | new | D06 |
| `send_again(job_id, profile_id) -> String` | new | D06 |
| `job_log(job_id, after_id, before_id, limit) -> Vec<LogEntry>` | new (the docs/06 draft `get_job_log`) | D07 |
| `activity(before_id, limit) -> Vec<ActivityItem>` | new | D07 |
| `service_log() -> Vec<String>` | new (protocol 25): Settings → "Copy log" | D07 |
| `decide_many(job_id, issue_ids, decision) -> u64` | new | D08 |
| `decide_all(…, query)`, `issues(…, query)`, `rule_counts(job_id, query)` | **changed**: optional path filter | D08 |
| `export_report(job_id, req) -> ExportResult` | new | D09 |
| `reclaim_status(job_id) -> ReclaimStatus` | new | D10 |
| `recheck_sources(job_id) -> ReclaimStatus` | new | D10 |
| `dismiss_reclaim(job_id, dismissed)` | new | D10 |
| `browse(target, dir, prefix) -> BrowseResult` | new | D11 |
| `check_folder_name(target, name) -> FolderNameCheck` | new | D11 |
| `last_target_dir(profile_id) -> Option<String>` | new (protocol 26) | D11 |
| `create_repair_job(profile_id, dir) -> String` | new (protocol 16), Phase 2 | [04](../04-name-correction.md#repair-destination-mode) |
| `set_profile_password(profile_id, password) -> Profile` | new (protocol 17), Phase 2 | [ADR 0004](../adr/0004-os-keyring-secrets.md) |
| `add_nextcloud_webdav(req) -> DetectNextcloudResponse` | new (protocol 18), Phase 2 | [05](../05-integration-nextcloud.md) |
| `install_app_key(profile_id)`, `remove_access(profile_id) -> AccessReport` | new (protocol 19), Phase 2 | [ADR 0012](../adr/0012-own-ssh-key-with-restrict.md) |

### Lifecycle rules (`crates/model/src/rules.rs`)

| Change | Spec |
|---|---|
| `JobGroup` (active, needs_you, not_sent, finished, cancelled) as pure data | D06 |
| `Op::SetLabel` (every state), `Op::SendAgain` (terminal states), `Op::Export` (terminal states) | D06, D09 |
| `EntryStatus::from_states`, `log_level(kind, data)`, `LOW_SPACE_*` constants | D05, D07, D03 |

### Contract changes (`crates/transport-api`)

| Change | Kind | Spec |
|---|---|---|
| `PreflightReport.total_bytes`, `used_bytes: Option<u64>` | additive fields | D03 |
| `StatsSnapshot.transferring: Vec<InFlight>` | additive field | D04 |
| `Transport::list` with an `Unsupported` default, `TransportCaps::LIST` | new method | D11 |

All of them are covered by the shared contract suite (`transport-api`, feature `contract`). None
needs anything in `core` beyond using the new fields, which keeps the rule "if adding a plugin
needs a change in `core`, the contract is wrong". `integration-api` doesn't change:
`Integration::preflight` already returns a `PreflightReport`, and D03 lets it carry a quota.

### Model events (`crates/model/src/event.rs`)

| Event | New / changed | Spec |
|---|---|---|
| `TransferProgress.in_flight: Vec<InFlightView>` | additive field | D04 |
| `EntriesDone { job_id, entries, more }` | new, batched, not persisted | D04 |
| `ReportProgress { job_id, rows, total }` | new | D09 |
| `ReclaimCheckProgress { job_id, checked, total }` | new | D10 |

### Window dependencies (`apps/desktop/package.json`)

| Package | Why | Spec |
|---|---|---|
| `@phosphor-icons/vue` | icons | [01](01-design-system.md#icons) |
| `@tanstack/vue-virtual` | headless list virtualisation | [01](01-design-system.md#long-lists) |
| `@tauri-apps/plugin-opener` | "Show in file manager" (reveal only) | D05, D10 |

## Status legend

- **Proposed**: written here, not on the roadmap yet.
- **Planned**: ticked into [09](../09-roadmap.md) for a phase.
- **Built**: merged; the spec stays as the reference and is updated if the code differs.
