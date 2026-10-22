# crates/secrets

OS keyring wrapper. **Built** in Phase 2.

## Responsibility
- `SecretStore` over the `keyring` crate: Keychain (macOS), Credential Manager (Windows),
  Secret Service (Linux).
- Opaque references `filebridge/<profile-id>/<kind>` (`app-password`, `ssh-key`).
- Generate Ed25519 SSH key pairs for profiles; expose the public key for `authorized_keys`.
- `Redacted<T>` wrapper whose `Debug`/`Display` print `***`.

## As built
- `SecretStore` trait; `KeyringStore` (the OS keyring) and `MemoryStore` (tests, fake transport).
- `reference(profile_id, SecretKind)`; `Secret = Redacted<Zeroizing<String>>`.
- Key generation lives with the SSH code (`russh`), not here: this crate only stores.
- `cargo test -p filebridge-secrets -- --ignored` runs a round trip on the real keyring.

## Allowed dependencies
`keyring`, `zeroize`, `filebridge-transport-api` (for `KeyringRef`).

See [docs/07-security-reliability.md](../../docs/07-security-reliability.md#security) and [ADR 0004](../../docs/adr/0004-os-keyring-secrets.md).
