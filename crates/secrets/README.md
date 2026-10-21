# crates/secrets

OS keyring wrapper. **Lands in:** Phase 2 (Phase 1 may use it early for the SSH key).

## Responsibility
- `SecretStore` over the `keyring` crate: Keychain (macOS), Credential Manager (Windows),
  Secret Service (Linux).
- Opaque references `filebridge/<profile-id>/<kind>` (`app-password`, `ssh-key`).
- Generate Ed25519 SSH key pairs for profiles; expose the public key for `authorized_keys`.
- `Redacted<T>` wrapper whose `Debug`/`Display` print `***`.

## Allowed dependencies
`keyring`, `ssh-key` (or similar), `zeroize`.

See [docs/07-security-reliability.md](../../docs/07-security-reliability.md#security) and [ADR 0004](../../docs/adr/0004-os-keyring-secrets.md).
