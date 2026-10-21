# ADR 0006 — Plugins compiled into the app in v1

- Status: Accepted
- Date: 2026-10-21

## Context
Extensibility comes from the `Transport`, `NameRule` and `Integration` contracts. Dynamic plugins
(e.g. WASM via Extism) add sandboxing, ABI and versioning costs.

## Decision
In v1, plugins are **Rust crates compiled into the app** and registered in a `Registry` at
startup. A new plugin = a new crate + one line in the registry setup. **Dynamic plugins (WASM with
Extism) only after the contracts stabilise.**

## Consequences
- Simple, fast, fully typed; refactoring contracts is cheap while they evolve.
- Third parties can't add plugins without rebuilding — acceptable for v1.
- Phase 3 gate checks the contracts are good enough: a new plugin must not change `core`.
