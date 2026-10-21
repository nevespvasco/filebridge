# ADR 0004 — Secrets only in the OS keyring

- Status: Accepted
- Date: 2026-10-21

## Context
The manual migration asked for the password on every reconnect, and left a root SSH key behind.
Secrets in config files are easy to leak.

## Decision
Store all credentials (Nextcloud app passwords, generated SSH private keys, any tokens) in the
**OS keyring** via the `keyring` crate (`crates/secrets`): Keychain on macOS, Credential Manager on
Windows, Secret Service on Linux. SQLite and config store only opaque references.

## Consequences
- No re-prompting; nothing secret on disk in plain text.
- Linux needs a Secret Service provider (GNOME Keyring / KWallet); headless CLI use needs a
  fallback (documented later, e.g. env var or encrypted file with explicit user opt-in).
- Logs use a `Redacted<T>` wrapper so secrets never reach them.
