# crates/transport-rclone

`Transport` implementation over **rclone's `rc` API**. **Lands in:** Phase 0.

## Responsibility
- Spawn and supervise the `rclone rcd` sidecar (localhost, random port, per-start rc user and
  password passed via environment, in-memory config, `--no-unicode-normalization`), restart it if
  it dies (restart: Phase 1).
- SFTP host keys are always checked against `known_hosts`. rclone is told to negotiate only the key
  types `known_hosts` holds for the host (`host_key_algorithms`, read with `russh`), since Go's SSH
  client otherwise fails with "key mismatch" when the server prefers a type that is not there.
- Configure backends in memory (`config/create` or connection strings) — local, SFTP, WebDAV
  (vendor `nextcloud`). No `rclone.conf` with secrets on disk.
- `transfer`: copy each item to its **mapped destination name** (`operations/copyfile` per file —
  speed validated in Phase 0 — or plan B: `sync/copy` to temp + batch rename + `operations/movefile`).
- Poll `core/stats` / `job/status` and forward to `ProgressSink`.
- `hash`: `operations/hashsum` (or SFTP remote `sha256sum`).
- Partial files hidden until complete.
- `set_bandwidth_limit`: `core/bwlimit` with `"<n>B"` or `"off"`. It is global to the rcd, so it is
  one app-wide limit that applies to running transfers too (ADR 0010).

## Capabilities (expected)
`RESUME`, `HASH_SHA256` (SFTP), `RENAME`, `PARTIAL_HIDDEN`; `CHOWN`/`CHMOD` not available over
SFTP without root → handled by the integration.

## Allowed dependencies
`transport-api`, `reqwest`, `tokio`, `serde_json`, `russh` (known_hosts parsing only). Never
`filebridge-secrets`: passwords and keys come through the `SecretResolver` the host injects
(`with_secrets`), and are scrubbed from rclone's errors and stderr while in use (`fs::Scrubber`).

See [ADR 0002](../../docs/adr/0002-rclone-behind-transport.md).
