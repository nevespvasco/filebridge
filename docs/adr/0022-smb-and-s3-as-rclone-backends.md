# ADR 0022 — SMB and S3 are rclone backends; `Backend` answers for itself so `core` never lists them

- Status: Accepted
- Date: 2026-10-23

## Context
Phase 3 adds SMB and S3 destinations ([09](../09-roadmap.md#phase-3--scale-more-destinations-and-tools)),
and its gate is that a new plugin enters without changing `core`
([03](../03-extension-contracts.md#phase-3-acceptance-test)).

rclone already speaks both (ADR 0002), with the same `rc` calls filebridge uses for SFTP and
WebDAV: `operations/copyfile`, `list`, `hashsum`, `movefile`, `about`. A separate crate per
protocol would repeat the whole rclone adapter (per-file copy, folder listings, hashing by
folder, secret scrubbing) to change one connection string.

A destination's backend is the `Backend` enum of `transport-api`, and `core` matched on it in
two places: where a profile's password goes (`set_profile_password`) and the backend's name in a
job's report. Both matches were exhaustive, so any new variant failed to compile in `core`: the
contract made `core` know every backend.

## Decision
- **SMB and S3 are new `Backend` variants** (`Smb`, `S3`), served by `transport-rclone`'s
  existing `RcloneTransport` (`:smb,…:share/path`, `:s3,…:bucket/path` connection strings). No
  new crate and no new `Registry` line: the transport id stays `rclone`.
- **`Backend` answers what callers need without listing its variants**: `kind()` (its `kind`
  tag, for reports and labels) and `password_mut()` (the slot of the one secret the user types:
  an SFTP or SMB password, a Nextcloud app password, an S3 secret access key; `None` for a local
  folder). `core` uses only these, so the next backend leaves `core` alone. Changing `core`'s
  two matches to them was the one-time contract fix; the SMB and S3 commit then touches no file
  in `crates/core`.
- **Secrets**: the SMB password and the S3 secret access key go to the keyring as the profile's
  `app-password` (ADR 0004), set by the same `set_profile_password` as SFTP's password. The S3
  access key id stays in the profile: it names the key, like a user name, and S3 servers print
  it in their errors and logs. rclone takes the SMB password obscured (`core/obscure`, like
  SFTP's) and the S3 secret as given; both are scrubbed from rclone's messages.
- **Profile fields**: SMB host, port (default 445), user, domain (rclone's default
  `WORKGROUP`), share; the root is a folder inside the share. S3 endpoint (empty = AWS), region,
  provider (rclone's name: `AWS`, `Minio`, `Ceph`…; default `Other`), bucket, access key id, and
  path-style addressing (on by default, which MinIO and most self-hosted servers need; off for
  virtual-hosted buckets); the root is a folder inside the bucket, with no leading `/`.
- **Capabilities stay the transport's**: neither server computes SHA-256 (SMB has no hashes,
  S3 only MD5/ETag), so verify downloads each file and hashes it locally, as for `rclone serve
  sftp` (ADR 0017). `LIST`, `MOVE` (S3: server-side copy then delete) and resume (rclone skips
  identical files) work as for the others.
- **Preflight**: S3 has no `about` (no free space to report); a destination that can't report
  space is checked by listing its root instead, so the connection test still says whether it is
  reachable and signed in, and the free space is unknown (as for WebDAV servers without quota).
- **Phones** (ADR 0014): librclone links the `smb` and `s3` backends too, so a phone can send to
  a NAS share or a MinIO. The S3 backend brings the AWS SDK, a few MB per ABI.

## Consequences
- `crates/core` doesn't change for SMB and S3, nor for a later rclone backend: a new backend is
  a `Backend` variant, its connection string in `transport-rclone`, its form and CLI command.
- The profile JSON gains two `kind`s; a window older than the service can't read such a profile
  (`PROTOCOL_VERSION` bumped).
- S3 keeps no empty folders and no folder modification times: a folder exists while a file is in
  it. Sends only create files, so nothing changes for them; "Repair destination" renames a
  folder by moving each file in it.
- SMB is tested against a real Samba only when `FILEBRIDGE_TEST_SMB_*` is set (rclone has no
  `serve smb`); S3 runs the contract suite against `rclone serve s3` in every test run.
