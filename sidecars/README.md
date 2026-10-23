# sidecars

The desktop app's Tauri sidecars, `filebridge-service` and **rclone**, and the pinned rclone
version. **Landed in:** Phase 1.

- `RCLONE_VERSION` (now `v1.75.2`): the one rclone version, for the desktop sidecar and for
  librclone on phones. Change it here; both scripts read it.
- `prepare-desktop.sh`: builds `filebridge-service` (release) and copies the pinned rclone into
  `sidecars/bin/`, which is **not committed** (see `.gitignore`). `pnpm bundle` in
  `apps/desktop` runs it first. rclone comes from `RCLONE`, `PATH` or `~/.local/bin/rclone`,
  and only if `rclone version` is the pinned one; get it from
  `https://downloads.rclone.org/<version>/`. *TODO: download it and check the release's
  `SHA256SUMS`, and a Windows (PowerShell) twin.*
- Tauri naming: `<name>-<target-triple>[.exe]`, e.g. `filebridge-rclone-x86_64-unknown-linux-gnu`,
  `filebridge-service-aarch64-apple-darwin`, `filebridge-rclone-x86_64-pc-windows-msvc.exe`.
  Tauri drops the triple when it installs them beside the app (`/usr/bin` in the .deb). rclone
  is `filebridge-rclone` so it never clashes with the distribution's `rclone` there;
  `filebridge_host::find_rclone` looks for that name first.
- rclone is **MIT-licensed**: its `COPYING`, kept here as `licenses/rclone-COPYING.txt`, goes
  into every bundle as a resource (`tauri.bundle.conf.json`).

## librclone (phones)

Phones can't run the sidecar, so they load rclone as a library ([ADR 0014](../docs/adr/0014-engine-in-the-app-on-phones.md)).
`librclone/build.sh` clones rclone at the same pinned version and builds `librclone/shim`
(rclone's own C shims with only the local, SFTP, WebDAV and SMB backends: ~22 MB instead of
~107 MB; not S3, which would add ~12 MB per ABI, [ADR 0022](../docs/adr/0022-smb-and-s3-as-rclone-backends.md)):

- `librclone/build.sh android`: `librclone.so` for `arm64-v8a` and `x86_64` into the Android
  app's `jniLibs` (needs Go and the NDK).
- `librclone/build.sh host`: `sidecars/bin/librclone/host/librclone.so`, for the transport
  contract test (`cargo test -p filebridge-transport-rclone --features librclone`).

See [ADR 0002](../docs/adr/0002-rclone-behind-transport.md).
