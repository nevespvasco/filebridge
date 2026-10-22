# crates/transport-nextcloud

`Transport` adapter that sends **small files to Nextcloud through its bulk upload**
(`POST /remote.php/dav/bulk`) and everything else through another transport, rclone in the app.
**Landed in:** Phase 1. See [ADR 0016](../../docs/adr/0016-nextcloud-bulk-upload.md).

## Responsibility
- `NextcloudBulk::over(inner, secrets)` answers to `inner`'s id and passes every method but
  `transfer` to it.
- `transfer`, for a WebDAV destination with vendor `nextcloud` whose server offers
  `dav.bulkupload` (asked once per server):
  - Files up to 4 MiB are candidates. Their folders are listed once through `inner`. A file
    already there with the same size and modification time (to the second) is done without
    sending. A name there only in another normalisation or case goes to `inner`. Missing folders
    are created with MKCOL.
  - Requests carry at most 100 files and 16 MiB, up to 5 at once (fewer when the job's
    concurrency is lower), within the app's bandwidth limit.
  - What bulk upload doesn't store goes to `inner` in one call, so its errors are classified as
    they always were. A refused sign-in (401) stops the batch as `Auth`.
- Progress: `ItemDone` per stored file; stats for the whole batch, `inner`'s added to bulk's.

Measured against Nextcloud 31 (Docker, PostgreSQL, 127.0.0.1), 1,200 small files through the
engine with an app password: 7.6 s (157 files/s), against 39 s through rclone alone
(`cargo run -p filebridge-host --example verify_bench`, `BENCH_BULK=0` for rclone alone).

## Wire format (`src/bulk.rs`)
One `multipart/related` part per file: `X-File-Path` (from the user's files root, raw UTF-8),
`X-File-MD5` (a wrong one fails the whole request with 400), `X-File-Mtime` (seconds),
`Content-Length`. The answer is JSON keyed by `X-File-Path`: `{"error": false, …}` or
`{"error": true, "message": …}`. Parent folders must exist.

## Tests
`tests/nextcloud.rs`: the contract suite through `NextcloudBulk` over rclone against
`rclone serve webdav` (all through rclone). Set `FILEBRIDGE_TEST_NEXTCLOUD_URL`
(`…/remote.php/dav/files/<user>/`), `_USER` and `_PASS` (an app password) to run it against a
real Nextcloud too, with 151 files sent twice (the second time all skipped) and a wrong password.

## Allowed dependencies
`transport-api`, `names` (`loose_key`), `reqwest` (rustls), `md5`, `getrandom`, `serde_json`,
`tokio`. Secrets come through the `SecretResolver` the host injects; never `filebridge-secrets`.
