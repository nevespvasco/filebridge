# sidecars

Per-platform **rclone** binaries shipped as Tauri sidecars. **Lands in:** Phase 0/1.

- Binaries go in `sidecars/bin/` and are **not committed** (see `.gitignore`); a script
  (Phase 1) downloads a pinned rclone version and verifies its checksum.
- Tauri naming: `rclone-<target-triple>[.exe]`, e.g. `rclone-x86_64-unknown-linux-gnu`,
  `rclone-aarch64-apple-darwin`, `rclone-x86_64-pc-windows-msvc.exe`.
- rclone is **MIT-licensed** — include its `LICENSE` in distributed builds.

## librclone (phones)

Phones can't run the sidecar, so they load rclone as a library ([ADR 0014](../docs/adr/0014-engine-in-the-app-on-phones.md)).
`librclone/build.sh` clones rclone at the same pinned version and builds `librclone/shim`
(rclone's own C shims with only the local, SFTP and WebDAV backends: ~19 MB instead of ~107 MB):

- `librclone/build.sh android`: `librclone.so` for `arm64-v8a` and `x86_64` into the Android
  app's `jniLibs` (needs Go and the NDK).
- `librclone/build.sh host`: `sidecars/bin/librclone/host/librclone.so`, for the transport
  contract test (`cargo test -p filebridge-transport-rclone --features librclone`).

See [ADR 0002](../docs/adr/0002-rclone-behind-transport.md).
