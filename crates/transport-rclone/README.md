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
  (vendor `nextcloud`), SMB and S3 ([ADR 0022](../../docs/adr/0022-smb-and-s3-as-rclone-backends.md)).
  No `rclone.conf` with secrets on disk. SMB's password goes obscured (`pass=`, like SFTP's and
  WebDAV's), S3's secret access key as given (`secret_access_key=`); both are scrubbed from
  rclone's messages by value and by parameter name.
- SMB: `:smb,host=…,user=…,domain=…:share/folder` (no user = `guest`). S3:
  `:s3,provider=…,endpoint=…,force_path_style=…:bucket/folder/`, a folder inside the bucket
  ending in `/` so rclone doesn't first ask whether it is a file (with a refused key it takes the
  refusal for "a file", and a wrong secret would read "is a file not a directory"); `hash` on
  S3 hashes the file through its folder with an include rule, for the same reason. S3 has no
  empty folders: `list` of a folder that holds nothing is `NotFound` (rclone lists it as empty).
- Errors: S3's `InvalidAccessKeyId` / `SignatureDoesNotMatch` are `Auth` (they come with a 403,
  which S3 also sends as `AccessDenied` for a bucket the key may not use: `Other`), `SlowDown` and
  `ServiceUnavailable` `Transient`, MinIO's `XMinioStorageFull` `Full`; SMB's logon failure is
  `Auth`, a share that isn't there (`Network Name Not Found`) `Other`.
- `transfer`: copy each item to its **mapped destination name** (`operations/copyfile` per file,
  plan A, ADR 0007; plan B was removed).
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
  (Nextcloud has no SHA-256, SMB no hash at all, S3 only MD5), rclone downloads the file and
  hashes it.
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
- `preflight`: `operations/about` for the free space; where the backend can't tell (S3, or an
  SMB folder that isn't there yet) the root is listed instead, so the test still proves the
  address and the sign-in: a folder not there yet is a warning (the first send creates it).
- Partial files hidden until complete.
- `set_bandwidth_limit`: `core/bwlimit` with `"<n>B"` or `"off"`. It is global to the rcd, so it is
  one app-wide limit that applies to running transfers too (ADR 0010).

## Tests
- `tests/contract.rs`: the shared contract against `rclone rcd` on the local backend, and over
  librclone with `--features librclone`, and over librclone linked in as on iOS with
  `--features librclone-linked` (`sidecars/librclone/build.sh host-archive` first; CI runs it
  inside an iOS simulator, ADR 0034).
- `tests/sftp_password.rs`, `tests/webdav.rs`, `tests/s3.rs`: against `rclone serve sftp`,
  `serve webdav` and `serve s3` on 127.0.0.1, secrets from a keyring: the contract, wrong secrets
  as `Auth`, no secret in any message.
- `tests/smb.rs`: rclone has no `serve smb`, so it runs only with `FILEBRIDGE_TEST_SMB_HOST`,
  `_SHARE`, `_USER`, `_PASSWORD` (and `_PORT`, `_DOMAIN`). Passed 2026-10-23 against Samba 4.23
  in Docker:

  ```bash
  docker run -d --rm --name fb-smb -p 127.0.0.1:14445:445 -e NAME=data -e USER=fbtest -e PASS=fbtest-pass dockurr/samba
  FILEBRIDGE_TEST_SMB_HOST=127.0.0.1 FILEBRIDGE_TEST_SMB_PORT=14445 FILEBRIDGE_TEST_SMB_SHARE=data \
    FILEBRIDGE_TEST_SMB_USER=fbtest FILEBRIDGE_TEST_SMB_PASSWORD=fbtest-pass \
    cargo test -p filebridge-transport-rclone --test smb
  docker stop fb-smb
  ```

## Capabilities (expected)
`RESUME`, `HASH_SHA256` (SFTP), `RENAME`, `PARTIAL_HIDDEN`; `CHOWN`/`CHMOD` not available over
SFTP without root → handled by the integration.

## Allowed dependencies
`transport-api`, `names` (`loose_key`, to compare a listed name with a destination name), `reqwest`, `tokio`, `serde_json`, `russh` (known_hosts parsing only). Never
`filebridge-secrets`: passwords and keys come through the `SecretResolver` the host injects
(`with_secrets`), and are scrubbed from rclone's errors and stderr while in use (`fs::Scrubber`).

See [ADR 0002](../../docs/adr/0002-rclone-behind-transport.md).
