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
- Every keyring call runs off the async threads through `filebridge_secrets::blocking`, which
  gives up after 60 s (a hung Secret Service, an unlock prompt nobody answers). An unreachable
  keyring (locked, absent, timed out: `SecretError::Unavailable`) is a **`Transient`** transport
  error, so a send waits and retries; only a secret that isn't stored is `Auth`.
- **Plugins never touch the keyring** (docs/03). `transport-api` defines `SecretResolver`
  (`KeyringRef` → `SecretValue`, read-only, zeroized, never printed); the host builds one over
  its `SecretStore` (`StoreResolver`) and injects it into the rclone transport and the Nextcloud
  SSH connector. Neither plugin crate depends on `crates/secrets`; only `core` (to store and
  delete) and the host do.
- rclone gets a password only obscured (`core/obscure`) inside the in-memory connection string,
  over its local authenticated rc API. **Obscuring is not encryption**: rclone's obscure is
  reversible by design, so an obscured password is as sensitive as the clear one. `SecretValue`
  wipes both copies when dropped; the `String`s inside rclone and in the JSON of the rc request
  are not wiped.
- Secrets are scrubbed from every rclone error (before it reaches a log, the database or the
  window) and from every line rclone writes to stderr (before `tracing`): the values of
  `pass=`/`key_pem=` parameters, quoted or in Go's `%q` form, and, while a call holds them, the
  exact values themselves (clear, obscured, PEM with real or `\n`-escaped line breaks, its body
  lines, each also as `%q`). Values under 4 characters are only caught by parameter.
- Deleting a profile deletes its secrets. (A deleted profile's row stays, marked deleted, so the
  transfers that used it keep their history: their report, file details and activity still
  read it. Saving a profile under a deleted id is refused.) Deleting is refused while a job to
  the profile is unfinished, drafts included. filebridge's own SSH key line stays on the server
  when a profile is deleted: the window says so and points to "Remove access" first.
- `set_profile_password(profile_id, password)` stores an SFTP password; the window's SFTP form and
  `filebridge profile set-password` (no echo, or stdin) use it. The changed profile is checked by
  its integration before the keyring is touched, so a Nextcloud WebDAV profile can't lose its
  app password. Passwords cross the IPC as `filebridge_ipc::Password`, whose `Debug` prints
  `***`: no request or envelope shows one, and they are never shown in a Details panel. If the
  SFTP form's password doesn't reach the keyring, a retry updates the profile it already saved.
