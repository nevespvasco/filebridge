# ADR 0016 — Small files to Nextcloud through its bulk upload, next to rclone

- Status: Accepted
- Date: 2026-10-22

## Context
Sending many small files to Nextcloud over WebDAV is bound by requests, not bytes. rclone's
WebDAV backend makes four requests per file: a PROPFIND to see if the file is there, a MKCOL of
its folder, the PUT, and a PROPFIND to read it back. Listing each folder once per batch removed
the first (`transport-rclone`, README). The other three are fixed inside rclone, with no option
to skip them.

Measured 2026-10-22 against Nextcloud 31 in Docker (PostgreSQL, 127.0.0.1), with an app password,
1,200 small files through the engine (`crates/host/examples/verify_bench.rs`):

| How | Sent in | Files/s |
|---|---|---|
| rclone, one `copyfile` per file (before) | 54 s | 22 |
| rclone, one folder listing per batch | 39 s | 30 |
| bulk upload, 3 requests of 100 files at once | 11.5 s | 104 |
| bulk upload, 5 requests of 100 files at once | 7.6 s | 157 |

Nextcloud's own bulk upload (`POST /remote.php/dav/bulk`, advertised as `dav.bulkupload` in its
capabilities since Nextcloud 23, used by its desktop client) takes many files in one
`multipart/related` request. Each part carries the file's path, its MD5 (checked by the server)
and its modification time, and the answer says per file whether it was stored.

[ADR 0002](0002-rclone-behind-transport.md) put rclone behind the `Transport` contract so it
"can be replaced per destination without touching `core`".

## Decision
- A new adapter crate, **`transport-nextcloud`** (`NextcloudBulk`), wraps the rclone transport.
  It answers to rclone's transport id and passes every method except `transfer` straight to it.
  Profiles keep `transport: "rclone"`, and `core` doesn't change. The host registers
  `NextcloudBulk::over(rclone)` in place of rclone.
- `transfer` uses bulk upload only for a WebDAV destination with vendor `nextcloud`, a
  `…/remote.php/dav/files/<user>/` address and a password, on a server whose capabilities offer it
  (asked once per server and remembered). Otherwise the whole batch goes to rclone.
- In a batch, files up to 4 MiB are candidates. Their folders are listed once through rclone:
  - a file already there with the same size and modification time is done without sending (as
    rclone would skip it);
  - a name that matches only in another normalisation or case goes to rclone;
  - folders that aren't there are created with MKCOL, top down.
- Candidates go in requests of at most 100 files and 16 MiB, up to 5 at once (fewer when the
  job's concurrency is lower), within the app's bandwidth limit (ADR 0010).
- Whatever bulk upload doesn't store goes to rclone in one call: a file the server refused, a
  request that failed, a folder that couldn't be listed or created. rclone classifies those errors
  (transient, not found, other) as it always has. Only a refused sign-in (401) stops the batch, as
  `Auth`, so a wrong password isn't tried once more by rclone.
- Progress: `ItemDone` when a request's answer says the file was stored. Stats count the batch
  from zero, adding rclone's own for the rest.

## Consequences
- Small files to Nextcloud go about 5× faster than through rclone (157 against 30 files/s), and
  about 30× the ~5 files/s first reported. That first number most likely came from signing in
  with the account password, which Nextcloud
  checks with a slow hash on every request (docs/05). With bulk upload that check costs once per
  100 files: 1,200 files in 14 s with the account password, 371 s before.
- A second HTTP client lives next to rclone, with TLS (rustls, the OS's certificates plus the
  Mozilla roots). It applies only to Nextcloud, and anything it can't do falls back to rclone.
- The bandwidth limit applies per request, not per byte: a request starts when its turn comes, so
  the rate averages out to the limit in steps of up to 16 MiB.
- A pause or a closed send window drops requests in flight. Their files stay `Partial` and go
  again.
- Not measured over the internet, on a server under other load, or on Android (it builds for
  `aarch64-linux-android`).
- Its contract tests: `rclone serve webdav` (generic WebDAV, so all through rclone), and a real
  Nextcloud when `FILEBRIDGE_TEST_NEXTCLOUD_URL`, `_USER` and `_PASS` are set.
