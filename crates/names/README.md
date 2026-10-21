# crates/names

The `NameRule` trait and the **general** name rules. **Lands in:** Phase 0 (NFC), Phase 1
(forbidden chars, trailing, reserved), Phase 2 (encoding, mojibake, collision, length).

## Responsibility
- `NameRule` trait, `Entry`, `Issue`, `Severity`, `TargetProfile`.
- General rules: `nfc`, `encoding`, `mojibake`, `forbidden-chars`, `trailing`, `reserved`
  (Windows set), `collision`, `length`.
- A `RuleSet` that runs active rules in the fixed order and returns the `source → destination`
  name map plus issues.

## Rules
- Pure, synchronous, no I/O — easy to unit-test with byte fixtures (NFD names, Latin-1 names,
  `PatolÃ³gica`, `CON`, trailing dots…).
- Mojibake is never automatic: always a proposal.

## Allowed dependencies
`unicode-normalization`, `encoding_rs`, `chardetng`. Nothing else in the workspace.

See [docs/04-name-correction.md](../../docs/04-name-correction.md).
