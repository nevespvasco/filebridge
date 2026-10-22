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

## As built (Phase 2)
- `crates/secrets`: `SecretStore` (`get`/`set`/`delete`), `KeyringStore` over `keyring` 3
  (Secret Service through libdbus on Linux, synchronous, so callers use `spawn_blocking`) and
  `MemoryStore` for tests and the fake-transport service. References are
  `filebridge/<profile-id>/<kind>` (`app-password`, `ssh-key`). Values are `Zeroizing` and
  `Redacted`.
- No fallback yet: without a keyring, a call fails with a message that says how to get one. The
  headless fallback stays open.
- rclone gets a password only obscured (`core/obscure`) inside the in-memory connection string,
  over its local authenticated rc API; `pass=`/`key_pem=` values are scrubbed from every rclone
  error before it reaches a log, the database or the window.
- Deleting a profile deletes its secrets. (A deleted profile's row stays, marked deleted, so the
  transfers that used it keep their history.)
- `set_profile_password(profile_id, password)` stores an SFTP password; the window's SFTP form and
  `filebridge profile set-password` (no echo, or stdin) use it. Requests carrying a password are
  never logged nor shown in a Details panel.
