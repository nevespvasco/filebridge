# crates/names

The `NameRule` trait and the **general** name rules. **Lands in:** Phase 0 (NFC), Phase 1
(forbidden chars, trailing, reserved), Phase 2 (encoding, mojibake, collision, length).

## Responsibility
- `NameRule` trait, `Entry`, `Issue`, `Severity`, `TargetProfile`, `ActiveFor`.
- General rules: `nfc`, `encoding`, `mojibake`, `forbidden-chars`, `trailing`, `reserved`
  (Windows set), `collision`, `length`.
- A `RuleSet` that runs active rules in the fixed order and returns the `source → destination`
  name map plus issues.

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
