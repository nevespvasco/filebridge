# crates/transport-rclone

`Transport` implementation over **rclone's `rc` API**. **Lands in:** Phase 0.

## Responsibility
- Spawn and supervise the `rclone rcd` sidecar (localhost, random port, per-start rc user and
  password passed via environment, in-memory config, `--no-unicode-normalization`), restart it if
  it dies (restart: Phase 1).
- Configure backends in memory (`config/create` or connection strings) — local, SFTP, WebDAV
  (vendor `nextcloud`). No `rclone.conf` with secrets on disk.
- `transfer`: copy each item to its **mapped destination name** (`operations/copyfile` per file —
  speed validated in Phase 0 — or plan B: `sync/copy` to temp + batch rename + `operations/movefile`).
- Poll `core/stats` / `job/status` and forward to `ProgressSink`.
- `hash`: `operations/hashsum` (or SFTP remote `sha256sum`).
- Partial files hidden until complete; bandwidth limit via `core/bwlimit`.

## Capabilities (expected)
`RESUME`, `HASH_SHA256` (SFTP), `RENAME`, `PARTIAL_HIDDEN`; `CHOWN`/`CHMOD` not available over
SFTP without root → handled by the integration.

## Allowed dependencies
`transport-api`, `reqwest`, `tokio`, `serde_json`.

See [ADR 0002](../../docs/adr/0002-rclone-behind-transport.md).
