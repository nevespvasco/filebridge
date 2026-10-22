# D08 — Issue selection in the Analysis step

- ID: D08 · Status: **Built** · Phase: 2
- Inspired by: F10 (Vault & Vine multi-select with a "1 Selected" bar), A6 (checkbox column),
  A8 (search over the table)
- Layers touched: `crates/store` (bulk decide by ids, path filter), `crates/core`
  (`decisions.rs`), `crates/model`, `crates/ipc` (`decide_many`, `issues` + `query`),
  `apps/desktop` (`IssueGroup.vue`, `SelectionBar`)
- Depends on: [01](../01-design-system.md) (`SelectionBar`, `SearchField`)

## Problem

`IssueGroup.vue` gives two granularities: one issue at a time (Accept / Keep original / Skip /
Edit on each row), or **every** pending issue of a rule (accept-all, keep-all, skip-all through
`decide_all(job_id, rule, decision)`). Nothing sits in between. With 2 000 collision issues
where the photos should get " (2)" and the `Documentos/Contratos` folder should be skipped, the
user clicks 2 000 times or accepts all and fixes the exceptions one by one (story S8).

## User stories

- S8: decide many name issues at once without accepting all of them.
- As the person migrating, I want to select every issue under one folder and decide them together.

## Design

```
COLLISIONS · 2 014 (1 870 pending)                        [Accept all] [Keep all] [Skip all]
[🔍 Filter by path…  Documentos/Contratos         ]   ☐ Select the 143 that match
┌──────────────────────────────────────────────────────────────────────────────┐
│ ☑ Documentos/Contratos/Ata.pdf          Ata.pdf → Ata (2).pdf   [pending]      │
│ ☑ Documentos/Contratos/Ata (1).pdf      …                      [pending]      │
│ ☐ Documentos/Contratos/Recibo.pdf       …                      [accepted]     │
│ …                                                                             │
└──────────────────────────────────────────────────────────────────────────────┘
┌──── 143 selected ─────────── [Accept] [Keep original] [Skip] ─── Clear ────┐   (sticky bar)
```

- Each row of an `IssueGroup` gets a checkbox. The header checkbox selects the loaded page; the
  link **Select the N that match** selects every issue of this rule that matches the current path
  filter, **including pages not loaded yet**.
- **Filter by path**: a `SearchField` per group. Case-insensitive substring on the source path;
  the query is NFC-normalised.
- `SelectionBar` appears while anything is selected: "{n} selected", then Accept, Keep original,
  Skip and Clear. **Edit** is not offered for a selection (one custom name can't apply to many).
- Selection is per rule group. Selecting in another group clears the first, with a short notice:
  "Selection cleared: one group at a time". Decisions are per Issue, and a cross-rule bulk
  decision would be hard to reason about.
- The existing bulk buttons stay, relabelled **Accept all / Keep all / Skip all** for the whole
  rule, as today.

## Behaviour

- Selection of loaded rows is a set of issue ids. "Select the N that match" is stored as a
  **query selection** (`rule` + path filter + "pending only?"), not as ids, so 100 000 matches
  never travel to the window.
- Applying:
  - Id selection → `decide_many(job_id, ids, decision)`.
  - Query selection → `decide_all(job_id, rule, decision, query)`: the existing operation gets
    an optional path filter.
- Already-decided issues in a selection are changed too (that's the point of selecting them), but
  the confirmation says so when it matters: "143 selected, 12 already decided: their decision
  will change". The confirmation is shown only when previous decisions would be overwritten.
- After applying, the group re-reads its counts (`rule_counts`) and the visible page. The
  selection clears, and the bar announces the result: "143 skipped".
- `Shift + click` selects a range of loaded rows; `Space` toggles the focused row
  ([02 — Keyboard](../02-information-architecture.md#keyboard)).

### As built

- "Select the N that match" selects the **pending** issues that match (it uses `decide_all`, which
  only changes pending ones), so the "their decision will change" confirmation isn't needed.
- `decide_many` answers a `DecideManyResult { changed, refused: [{ id, reason }] }` instead of a
  `partial` error: what it could apply is applied, and the window lists the first refusal.
- The path filter matches like the store's `LIKE`: a substring, ASCII letters in any case. The
  engine's own pre-check uses the same rule, so the two never disagree on which issues match.

## States

| State | Behaviour |
|---|---|
| Nothing selected | No bar |
| Applying | Bar buttons disabled, with a spinner on the one pressed |
| Error | The bar stays with the selection intact and an inline error; nothing is half-applied (one transaction) |
| Job left `AwaitingApproval`/`Ready` meanwhile | Disabled with the reason, per `JOB_OPERATIONS` (`decide`) |

## Data and contracts

### IPC

```text
/// The same decision for each listed issue of the job. Not `edit`. Answers the number changed.
DecideMany decide_many(job_id: String, issue_ids: Vec<i64>, decision: DecisionInput) -> u64;
/// changed: optional `query` (substring of the source path) to limit which issues are decided.
DecideAll decide_all(job_id: String, rule: Option<String>, decision: DecisionInput, query: Option<String>) -> u64;
/// changed: optional `query` to filter the page by source path.
Issues issues(job_id: String, rule: Option<String>, after_id: Option<i64>, limit: u32, query: Option<String>) -> Vec<IssueView>;
/// changed: optional `query` so the "Select the N that match" link knows N.
RuleCounts rule_counts(job_id: String, query: Option<String>) -> Vec<RuleCount>;
```

- `decide_many`: at most 10 000 ids per call (the window splits bigger id sets; with the query
  selection they are rare).
- Both run under `Op::Decide` (allowed in `AwaitingApproval` and `Ready`, as today) and go
  through the Name plan's `check` like single decisions, so a Skip or Keep original that would
  create a **collision** is refused for that issue. The answer counts only the issues changed,
  and the refused ones are listed in an error body `{ code: "partial", refused: [{id, reason}] }`.

### Store

- `decide_issues(job_id, ids, decision)`: one transaction, `UPDATE issues SET decision = ?,
  custom_name = NULL, decided_at = ? WHERE job_id = ? AND id IN (SELECT value FROM json_each(?))`.
- The path filter joins `manifest_entries` on `(job_id, entry_idx)` (index from M-entry-index) and
  matches `CAST(raw_path AS TEXT) LIKE ? ESCAPE '\'`.

### Core

`decisions.rs` gets `decide_many`, sharing the per-issue validation with `decide`. It appends one
`decisions` log event per call ([D07](D07-job-log-and-activity.md)).

## i18n

| Key | en | pt |
|---|---|---|
| `issues.filter` | Filter by path… | Filtrar por caminho… |
| `issues.selectMatching` | Select the {n} that match | Selecionar os {n} que correspondem |
| `issues.selected` | {n} selected | {n} selecionados |
| `issues.overwrite` | {n} selected, {m} already decided: their decision will change | {n} selecionados, {m} já decididos: a decisão deles vai mudar |
| `issues.oneGroup` | Selection cleared: one group at a time | Seleção limpa: um grupo de cada vez |
| `issues.applied.skip` | {n} skipped | {n} ignorados |
| `issues.partial` | {n} couldn't change: {reason} | {n} não puderam mudar: {reason} |

## Accessibility

- Checkboxes are labelled with the row's path ("Select Documentos/Contratos/Ata.pdf").
- The bar is a `role="region"` labelled "Selection", and its count is a polite live region.
- Range selection also works from the keyboard (`Shift + Space` from the anchor).

## Acceptance criteria

1. In a fixture with 300 collision issues, filtering "Contratos" and **Select the N that match**
   then **Skip** skips exactly the matching issues, including those on unloaded pages, in one
   request.
2. Selecting 3 rows by checkbox and pressing **Accept** sends one `decide_many` with those 3 ids.
3. A **Keep original** on a selection where one issue would collide with another name refuses that
   one, applies the rest, and shows "1 couldn't change: …".
4. Selecting in a second group clears the first selection and says so.
5. One `decisions` event is logged per bulk call, with its count.

## Out of scope

- Bulk **Edit** (a pattern like "replace ':' with '-' in the selection"). That would be a new
  Name rule or a rule option, not a selection feature.
- Selecting across rule groups.
