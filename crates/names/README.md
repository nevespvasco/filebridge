# crates/names

The `NameRule` trait and the **general** name rules. **Lands in:** Phase 0 (NFC), Phase 1
(forbidden chars, trailing, reserved), Phase 2 (encoding, mojibake, collision, length).

## Responsibility
- `NameRule` trait, `Entry`, `Issue`, `Severity`, `TargetProfile`, `ActiveFor`.
- General rules: `nfc`, `encoding`, `mojibake`, `forbidden-chars`, `trailing`, `reserved`
  (Windows set), `collision`, `length`.
- A `RuleSet` that runs active rules in the fixed order and returns the `source → destination`
  name map plus issues.
- The **Name plan** (`NamePlan`): entries plus the user's decisions in, the final destination path
  (or skip) of every entry out. `core` only persists what it returns.

## Name plan

One module owns every step from raw name bytes to the destination path, so the rules hold for
names the user keeps or types too:

| Call | When | Does |
|---|---|---|
| `propose(entries)` | Analysis (stage 3) | Runs the rules; adds an `encoding` issue for a name no rule could decode and a `collision` issue (with a ` (2)` suffix proposal) for a path already taken |
| `check(entry, rule, choice)` | The user decides | Refuses a choice that can't work: Accept on an undecodable name, an edited name an active rule would still change, or Keep original when another rule would then change the name (Nextcloud's own NFC rule once the general one is kept out) |
| `resolve(entries, decisions)` | Send | Re-runs the rules with the decisions applied: Keep original leaves out **only that rule**, Edit replaces the name, Skip skips the entry and everything under it, an accepted collision takes its suffix. Fails on pending decisions and on two entries landing on one path |

`Choice` is the user's decision (`Accept`, `KeepOriginal`, `Edit(name)`, `Skip`); `core` maps the
store's `Decision` to it in one place (`crates/core/src/decisions.rs`).

## What exists now

| Rule | Type | Fix | Active on |
|---|---|---|---|
| `nfc` | `Nfc` | Compose to NFC | Every target |
| `forbidden-chars` | `ForbiddenChars` | `< > : " \ \| ? *` and 0x00–0x1F, 0x7F → `_` | `ActiveFor` given (default: Windows) |
| `trailing` | `Trailing` | Strip trailing spaces and dots (all-dots/spaces → `_`) | Same `ActiveFor` as `forbidden-chars` |
| `reserved` | `Reserved` | Prefix `_` | `Reserved::windows()`: device names on Windows |

All four are `Severity::Auto` and only act on names that are valid UTF-8.

- `general_rules(windows_compat)` returns the four, with `forbidden-chars` and `trailing` sharing
  `windows_compat` (Windows, plus any integration id that requires them, e.g.
  `ActiveFor::windows().and_integrations(["nextcloud"])`).
- `RuleSet::new` drops rules whose `applies_to` is false and **sorts the rest by id into
  `RULE_ORDER`** (`encoding` → `mojibake` → `nfc` → `forbidden-chars` → `trailing` → `reserved` →
  `length` → `collision`), so callers don't have to get the order right. The sort is stable; ids
  not in `RULE_ORDER` (integration rules) run right after `reserved`.
- `Reserved` matches a list entry ignoring ASCII case against the whole name or the name up to
  its first dot (`CON`, `con.txt`, `nul.tar.gz`). An integration reuses it with its own list:
  `Reserved::new([".htaccess"], ActiveFor::integrations(["nextcloud"]))`. Extension-style entries
  (`.part` meaning "any name ending in `.part`") are not supported yet: they need a suffix match
  and a different fix than a `_` prefix.

## Rules
- Pure, synchronous, no I/O — easy to unit-test with byte fixtures (NFD names, Latin-1 names,
  `PatolÃ³gica`, `CON`, trailing dots…).
- Mojibake is never automatic: always a proposal.

## Allowed dependencies
`unicode-normalization`, `encoding_rs`, `chardetng`. Nothing else in the workspace.

See [docs/04-name-correction.md](../../docs/04-name-correction.md).
