# sidecars

The desktop app's Tauri sidecars, `filebridge-service`, the `filebridge` CLI and **rclone**, and
the pinned rclone version. **Landed in:** Phase 1; the CLI, the download and Windows in Phase 3.

- `RCLONE_VERSION` (now `v1.75.2`): the one rclone version, for the desktop sidecar and for
  librclone on phones. Change it here; both scripts read it.
- `prepare-desktop.sh` (Linux, macOS) and `prepare-desktop.ps1` (Windows, PowerShell 5.1 or 7):
  build `filebridge-service` and the CLI (release) and put them and the pinned rclone into
  `sidecars/bin/`, which is **not committed** (see `.gitignore`). The bash one also writes the
  CLI's man pages (gzipped) and bash/zsh/fish completions to `sidecars/bin/share/`, which the
  .deb and .rpm install. `pnpm bundle` in `apps/desktop` runs the one for the OS first
  (`apps/desktop/scripts/prepare-sidecars.mjs`). rclone comes from, in this order:
  - **`RCLONE_DOWNLOAD=1`**: the pinned release for the *target*
    (`https://downloads.rclone.org/<version>/rclone-<version>-<os>-<arch>.zip`), so a build
    machine needs no rclone of its own and a cross build gets the right one. Its SHA-256 must
    match the release's `SHA256SUMS` (only its PGP-signed part is read) before anything is
    taken out of the zip or run; a mismatch deletes the download and stops. When `gpg` has
    rclone's signing key (`FBF737ECE9F8AB18604BD2AC93935E02FF3B54FA`, e.g. `gpg --fetch-keys
    https://rclone.org/KEYS`) the signature on `SHA256SUMS` is checked too, and a bad one
    stops; without the key it says it wasn't checked, unless `RCLONE_REQUIRE_SIGNATURE=1`
    makes that an error. Kept in `sidecars/bin/download/<version>/` for the next build.
  - `RCLONE=/path/to/rclone`: that file.
  - otherwise `rclone` on `PATH`, else `~/.local/bin/rclone` (bash only).

  A local rclone is used only when `rclone version` is the pinned one (not checkable for
  another target, so a cross build with `RCLONE` trusts it).
  ```bash
  RCLONE_DOWNLOAD=1 pnpm bundle          # in apps/desktop
  ```
  ```powershell
  $env:RCLONE_DOWNLOAD = "1"; pnpm bundle
  ```
- Tauri naming: `<name>-<target-triple>[.exe]`, e.g. `filebridge-rclone-x86_64-unknown-linux-gnu`,
  `filebridge-service-aarch64-apple-darwin`, `filebridge-x86_64-pc-windows-msvc.exe`.
  Tauri drops the triple when it installs them beside the app (`/usr/bin` in the .deb, where
  the CLI is `/usr/bin/filebridge` and the app `filebridge-desktop`). rclone is
  `filebridge-rclone` so it never clashes with the distribution's `rclone` there;
  `filebridge_host::find_rclone` looks for that name first.
- rclone is **MIT-licensed**: its `COPYING`, kept here as `licenses/rclone-COPYING.txt`, goes
  into every bundle as a resource (`tauri.bundle.conf.json`).

## librclone (phones)

Phones can't run the sidecar, so they load rclone as a library ([ADR 0014](../docs/adr/0014-engine-in-the-app-on-phones.md)).
`librclone/build.sh` clones rclone at the same pinned version and builds `librclone/shim`
(rclone's own C shims with only the local, SFTP and WebDAV backends: ~19 MB instead of ~107 MB):

- `librclone/build.sh android`: `librclone.so` for `arm64-v8a` and `x86_64` into the Android
  app's `jniLibs` (needs Go and the NDK).
- `librclone/build.sh host`: `sidecars/bin/librclone/host/librclone.so`, for the transport
  contract test (`cargo test -p filebridge-transport-rclone --features librclone`).

See [ADR 0002](../docs/adr/0002-rclone-behind-transport.md).
