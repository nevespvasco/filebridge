# ADR 0013 — Phones as a Source, Android first

- Status: Accepted
- Date: 2026-10-22
- Amends: [ADR 0001](0001-tauri-vue-rust.md) (platforms: Linux, Windows, macOS); reverses D12 in the
  [design review](../design/00-reference-review.md) ("a narrow desktop window, not a phone app")

## Context
filebridge was scoped as a desktop app. The author wants to use it on their Android phone too, and
later on an iPhone. A phone can play two different roles:

- (a) **a Source**: the phone's own files are sent to a Profile, with the full engine on the phone;
- (b) **a remote control** for a computer's `filebridge-service`: watch a long send, answer
  issues, pause and resume.

(b) needs the service reachable over the network, which ADR 0008 deliberately rules out (only the
local user can connect). iOS needs a Mac, an Apple Developer account, and allows little background
work; Android allows long work through a foreground service.

## Decision
- On phones filebridge is **(a), the same app with the phone's files as Sources**. (b) is out of
  scope and would need its own ADR.
- **Android first** (Phase 4a), **iOS after** (Phase 4b), and iOS only once a Mac and an Apple
  Developer account are available.
- **Personal builds**: a signed APK installed by hand. Store releases wait for signed installers.
- *Amended (2026-10-22):* the author asked to build Phase 4a now, alongside Phase 2, rather than
  after its gate.
- *Amended by [ADR 0034](0034-ios-without-a-mac.md) (2026-10-26):* iOS no longer waits for a Mac
  or a paid account: it is built on CI's Mac and signed with a free Apple ID.

Details in [11 — Mobile](../11-mobile.md).

## Consequences
- The Job model, the Manifest, verification and the Report stay the same on every platform.
- Personal builds allow Android's "All files access", so Sources stay plain paths and `scanner` is
  unchanged. A store release would need the Storage Access Framework and a new kind of Source.
- On a phone only folders are picked (the system folder picker gives a tree, not loose files),
  one at a time. *Amended 2026-10-27:* files too, several at once, because the author asked for
  it after trying the phone app. On Android the file picker's document ids give the path for the
  phone's storage and SD cards (`primary:…`, `raw:…`), and MediaStore gives it for the Images,
  Videos, Audio and Downloads views ("All files access" lets the app read it). Cloud files have
  no path: they are left out, and the app says how many. On iOS each picked file gets its own
  security-scoped bookmark, like a folder. Sources are still plain paths, and `scanner` is
  unchanged.
- The narrow layouts from D12 become the phone UI; desktop-only parts (tray, service status, drag
  & drop, "Open in file manager") are hidden there.
- Rejected: the phone as a remote control (network-exposed service); iOS first (higher cost, weaker
  background support, no device to test); store releases now (signing and permission review).
