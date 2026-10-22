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
  Each destination folder that two or more of the batch's files go to is listed once
  (`operations/list`); a file it doesn't hold under any spelling of its name (`names::loose_key`:
  another normalisation or case counts as there) is copied with `NoCheckDest`, skipping rclone's
  per-file existence check. Files that are there, and folders that hold one file or can't be
  listed, keep the check, so an identical file is still skipped. Over WebDAV rclone makes four
  requests per file (PROPFIND to check, MKCOL of the folder, PUT, PROPFIND to read back); this
  drops the first. The other three are rclone's own.
  Measured 2026-10-22 against Nextcloud 31 (Docker, PostgreSQL, 127.0.0.1) with an app password,
  1,200 small files through the engine (`--example verify_bench`, concurrency 8): sent in 54 s
  before (22 files/s), 40 s after (30 files/s). The same send signed in with the **account
  password** took 371 s (3.2 files/s): Nextcloud checks an account password with a slow hash on
  every request (~175 ms for an empty MKCOL, ~43 ms with an app password). Concurrency stops
  paying at 8 there (copyfile alone: 17 files/s at 4, 23 at 8, 20 at 16).
- Poll `core/stats` / `job/status` and forward to `ProgressSink`.
- `hash`: `operations/hashsum` (or SFTP remote `sha256sum`); when the server can't hash
  (Nextcloud has no SHA-256), rclone downloads the file and hashes it.
- `hash_many` (verify): one `operations/hashsum` per folder, limited to the asked names by
  `_filter` include rules (escaped globs) and `MaxDepth 1`. rclone lists a file's whole folder to
  find it, so asking per file cost one folder listing (twice: hash, then download) per file —
  about one file per second against Nextcloud. Server hashes first, then downloads, 8 at a time
  (`Transfers`); a folder whose server hashed nothing means the next folders skip that try.
  Whatever the folder call doesn't answer (a missing file, a non-transient error) is asked of
  `hash`, so each answer is the one `hash` gives.
  Measured 2026-10-22 against Nextcloud 31 (Docker, 127.0.0.1), 1,200 small files sent and
  verified by the engine (`cargo run -p filebridge-host --example verify_bench`): verify took
  996 s before (1.2 files/s) and 56 s after (21 files/s), 1,200 verified and 0 mismatched both
  times. `--example hash_compare` on those files, 5 changed and 3 deleted on the server: the same
  answer from `hash_many` and from `hash` for all 1,202 paths, 88 s against 905 s. Those runs
  most likely signed in with the account password (their send rate, 4–6 files/s, is what the
  account password gives on that server; an app password gives 23); with an app password the
  same verify takes 15–18 s (~70 files/s).
- Partial files hidden until complete.
- `set_bandwidth_limit`: `core/bwlimit` with `"<n>B"` or `"off"`. It is global to the rcd, so it is
  one app-wide limit that applies to running transfers too (ADR 0010).

## Capabilities (expected)
`RESUME`, `HASH_SHA256` (SFTP), `RENAME`, `PARTIAL_HIDDEN`; `CHOWN`/`CHMOD` not available over
SFTP without root → handled by the integration.

## Allowed dependencies
`transport-api`, `names` (`loose_key`, to compare a listed name with a destination name), `reqwest`, `tokio`, `serde_json`, `russh` (known_hosts parsing only). Never
`filebridge-secrets`: passwords and keys come through the `SecretResolver` the host injects
(`with_secrets`), and are scrubbed from rclone's errors and stderr while in use (`fs::Scrubber`).

See [ADR 0002](../../docs/adr/0002-rclone-behind-transport.md).
