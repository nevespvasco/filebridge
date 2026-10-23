# 11 — Mobile (Android, then iOS)

*Android built (Phase 4a); iOS not started.* The plan to run filebridge on phones: what it is for there, what changes in the
engine, and the order of the work. Decisions: [ADR 0013](adr/0013-phones-as-a-source-android-first.md)
(scope) and [ADR 0014](adr/0014-engine-in-the-app-on-phones.md) (how the engine runs). Roadmap:
[Phase 4](09-roadmap.md#phase-4--mobile).

## What filebridge is on a phone

The same app, with the phone's own files as Sources: pick a folder (the camera roll, Downloads, a
WhatsApp media folder, an SD card), pick a Profile, review the names, send, verify, update the
tool. The wizard, the Job lifecycle, the Manifest, verification and the Report do not change.

Not in scope: the phone as a remote control for a computer's `filebridge-service`. That needs the
service on the network, which [ADR 0008](adr/0008-service-ipc.md) rules out today (only the local
user can connect). It can be its own phase later, with its own ADR (pairing, authentication).

Order: **Android first** (Phase 4a), **iOS second** (Phase 4b, only once a Mac and an Apple
Developer account are available). Builds are for personal use: a signed APK installed by hand,
like the desktop builds ([10](10-risks-and-open-decisions.md#risks)). Stores wait for signed
installers (Phase 3).

## What is already portable

- `core`, `model`, `names`, `store` (rusqlite `bundled`), `scanner`, `ipc`'s operation catalogue and
  the Vue UI compile for `aarch64-linux-android` and `aarch64-apple-ios` without changes. Both
  targets are `cfg(unix)`, so the `OsStrExt` paths for raw names work.
- The UI already fits from 360px ([design 02](design/02-information-architecture.md#window-sizes)).
- The Tauri commands call the `Operations` trait, which two implementations already satisfy: the
  IPC client (desktop window) and `host::EngineOperations` (the CLI, in-process).
- `transport-rclone` reaches rclone through one function, `RcClient::call(method, json) -> json`,
  which has the same shape as librclone's `RcloneRPC(method, json)`.
- Sleep inhibit already falls back to a logged no-op on any other OS.

## What has to change

| Area | Desktop today | Phone | Where |
|---|---|---|---|
| Where the engine runs | `filebridge-service`, a detached process; the window is an IPC client | Inside the app process, through `host::EngineOperations` ([ADR 0014](adr/0014-engine-in-the-app-on-phones.md)) | `apps/desktop/src-tauri` (`cfg(mobile)`) |
| Keeping a send alive | The service outlives the window; `systemd --user` on Linux | Android: a foreground service with a progress notification keeps the process alive. iOS: sends advance while the app is open, then resume on return | Tauri mobile plugin (Kotlin / Swift) |
| rclone | `rclone rcd` child process over HTTP on localhost | librclone linked in-process (Go `c-shared` on Android, `c-archive` on iOS), called through its C `RcloneRPC` | `transport-rclone`: an `RcCall` trait, implemented by `Rcd` and `Librclone` |
| Picking Sources | `tauri-plugin-dialog` folder picker and drag & drop; absolute paths | Android: "All files access" (`MANAGE_EXTERNAL_STORAGE`), so `/storage/emulated/0/…` paths work with `std::fs` and rclone `:local:`; a small folder picker. iOS: folder picker with security-scoped bookmarks, kept open while a Job reads them | Tauri mobile plugin; `scanner` unchanged |
| Secrets | `keyring` 3.x: Keychain, Credential Manager, Secret Service | iOS: Keychain (`apple-native` covers iOS). Android: a Keystore-backed store, through `keyring`'s pluggable credential builder or `keyring-core` | `crates/secrets` |
| SSH known hosts, SSH agent | `~/.ssh/known_hosts`; the user's key file or agent | known hosts in the app's data folder; no agent: only filebridge's own key from the keyring ([ADR 0012](adr/0012-own-ssh-key-with-restrict.md)) | `transport-rclone/src/fs.rs`, `integration-nextcloud/src/ssh.rs` |
| Data folder | `HOME` / `XDG_DATA_HOME` / `APPDATA` | The app's data folder from Tauri (`app_data_dir`) passed to `Host::open` | `crates/host` |
| Sleep inhibit | `systemd-inhibit`, `caffeinate`, `SetThreadExecutionState` | Android: partial wake lock and Wi-Fi lock held by the foreground service. iOS: idle timer off while sending in the foreground | The app (`engine.rs`); `core`'s inhibit is a quiet no-op on phones |
| Desktop-only UI | Tray, close-to-tray, drag & drop, service status, "Install as service", "Open in file manager" | Hidden. A reclaimable Source shows its path instead of opening a file manager (ADR 0011 still holds: the app never frees it) | Vue, by a `platform` flag from Rust |

## Destinations on a phone

| Profile | Phone | Note |
|---|---|---|
| Nextcloud over WebDAV (app password) | Yes, first | No SSH needed; the natural first target |
| SFTP | Yes | filebridge's own key only (no agent on a phone) |
| Nextcloud direct mode (SSH + `occ`, `pct`) | Yes, after SFTP | russh runs on both targets; `aws-lc-sys` needs the NDK / Xcode C toolchain, or russh's `ring` backend |
| Local disk (`Backend::Local`) | No | A USB drive on a phone is out of scope |

## The layering rule on phones

`CLAUDE.md` says the UI reaches `core` only through `ipc`. On a phone that stays true at the
contract level: Tauri commands still call `ipc`'s `Operations`; only the implementation behind it
changes (`EngineOperations` instead of the socket client). The desktop window keeps the socket.
When Phase 4a starts, `CLAUDE.md` and [08](08-repository-structure.md) get one line saying the
mobile build wires `crates/host` into the app.

## Order of work

### Phase 4a — Android

1. **Seams on desktop first** (no phone needed, all testable with `cargo test`): an `RcCall`
   trait in `transport-rclone` (`Rcd` = today's HTTP client); the data folder from the app;
   `HOME` in the app's data folder, so known hosts live there too; the Tauri app chooses its
   `Operations` by `cfg(mobile)`.
2. **librclone build**: a `sidecars/librclone` script that cross-compiles librclone for Android
   (Go + NDK clang, `-buildmode=c-shared`), and `Librclone` behind a `librclone` feature. Its
   contract suite runs on desktop against a host build of librclone, so the same tests cover
   both.
3. **Tauri Android project**: `tauri android init`, mobile capabilities, the Android plugin
   (foreground service, wake/Wi-Fi locks, "All files access" prompt, folder picker).
4. **Android Keystore store**: the plugin's vault, behind `SecretStore` in the app.
5. **UI pass**: hide desktop-only parts; check every screen at 360–412px on a device.
6. **Real run** against the gate.

**Gate:** from an Android phone, send a real phone folder with at least 1,000 photos and videos
to Nextcloud over WebDAV; turn the screen off and switch apps mid-send; force-stop the app once
and see the Job resume; finish with a verification of zero differences.

### Phase 4b — iOS

Starts only with a Mac (Xcode) and an Apple Developer account. librclone as `c-archive` for
`aarch64-apple-ios` (upstream says iOS is untested), `tauri ios init`, a Swift plugin for the
folder picker and security-scoped bookmarks, Keychain via `apple-native`.

**Gate:** send a folder from the Files app to Nextcloud over WebDAV, leaving the app and coming
back mid-send; the Job resumes and the verification has zero differences.

## As built (2026-10-22)

| Piece | Where |
|---|---|
| `RcCall` trait; `Rcd` (HTTP) and `Librclone` (FFI, feature `librclone`, loaded with `libloading`); both pass the transport contract suite | `crates/transport-rclone` |
| librclone build: rclone's C shims with the local, SFTP, WebDAV and SMB backends only (~22 MB per ABI; SMB since ADR 0022, S3 left out), plus `RcloneSetenv` | `sidecars/librclone/` |
| `TransportChoice::Librclone`, `Host::open_with_secrets` | `crates/host` |
| The engine in the app under `cfg(mobile)`, same commands and events as the desktop's `Service`; the foreground service follows the busy Jobs | `apps/desktop/src-tauri/src/engine.rs` |
| Phone-only commands (`platform`, `pick_folder`, `storage_access`, `request_storage_access`, `request_battery_exemption`, `set_send_notice_labels`) and the Keystore `SecretStore` | `apps/desktop/src-tauri/src/native.rs`, `commands.rs` |
| Android plugin: folder picker → path, "All files access", `SendService` (`specialUse`, wake and Wi-Fi locks), `SecretVault` (AES-GCM key in the Android Keystore) | `gen/android/app/src/main/java/app/filebridge/` |
| UI: folders only, no drag & drop or "show in file manager", the notification's text in pt/en, battery exemption asked once at the first send | `apps/desktop/src/lib/platform.ts`, `FileDropZone.vue`, `App.vue` |
| Phone texts: a `phone` section in `en.json`/`pt.json` laid over the normal texts on a phone ("You can leave the app: it keeps going" instead of the window) | `apps/desktop/src/i18n/index.ts` (`applyPhoneTexts`), called from `initPlatform()` |
| Package id `app.filebridge` on Android only (`identifier` override). Desktop keeps `app.filebridge.desktop`: there it names the folder of the UI's local settings, and changing it would reset them. The service data (`~/.local/share/filebridge`) and the keyring service (`filebridge`) never depended on it | `apps/desktop/src-tauri/tauri.android.conf.json` |
| Release APK signed with your own key: the one-time `keytool -genkeypair` command is in the script's header; Gradle reads the keystore, alias and passwords from `FILEBRIDGE_ANDROID_*`; with no keystore the debug key signs | `apps/desktop/scripts/android-release.sh`, `gen/android/app/build.gradle.kts` |
| `quietconfig`: runs before rclone's config setup so librclone no longer logs "Couldn't find configuration directory" | `sidecars/librclone/shim/quietconfig/` |

**Tested** on an Android 15 emulator (x86_64), not yet on a real phone: 1,201 photos (37.5 MB,
one NFD name) from `DCIM/Camera` to Nextcloud 31 over WebDAV, with the app in the background and
the screen off, force-stopped at 288 files, reopened, resumed and finished; verification 1,201
identical, 0 differences; the foreground service stopped by itself at the end. The app password
was stored only as ciphertext.

**Known gaps:**

- **SSH profiles on a phone** (SFTP, Nextcloud direct mode) are not yet tried on one. The
  "trust this host key" step ([ADR 0015](adr/0015-trust-unknown-host-key-in-the-app.md)) is in
  the shared `Operations`, so the phone build has it, and it writes to the `known_hosts` in the
  app's data folder. A phone has no SSH agent: sign in with filebridge's own key
  ([ADR 0012](adr/0012-own-ssh-key-with-restrict.md)) or a key file.
- rclone picks its cache folder before the app sets HOME. The local, SFTP and WebDAV backends
  don't use it.
- An emulator or phone that had the earlier `app.filebridge.desktop` build keeps it as a
  separate app: uninstall it by hand.
- iOS (Phase 4b) is not started: it needs a Mac.

## Risks

| Risk | Impact | Mitigation |
|---|---|---|
| librclone is marked experimental; iOS untested upstream | High | The transport contract suite runs both `Rcd` and `Librclone`; plan B is a native SFTP/WebDAV transport crate (a new plugin, no `core` change) |
| Go cross-compiling (NDK, `CGO`) adds a second toolchain | Medium | One script in `sidecars/`, pinned Go and NDK versions |
| "All files access" is hard to get approved on Google Play | Low now | Personal APK only; the Storage Access Framework (content URIs) would need a new Source kind, which waits for a store release |
| iOS stops background work after a short time | Medium | Accepted: sends advance in the foreground; every Stage is already persisted, so a Job resumes where it stopped |
| Android kills a foreground service under memory pressure or battery savers | Medium | Same resume path as a crash (`kill -9` test from Phase 1); ask to exclude filebridge from battery optimisation |
| A keyring store for Android does not exist in `keyring` 3.x | Medium | Write it behind `keyring`'s credential builder, backed by Android Keystore through the Kotlin plugin |
