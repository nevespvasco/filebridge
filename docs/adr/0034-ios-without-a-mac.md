# ADR 0034 — iOS is built on CI's Mac and tested without a paid Apple account

- Status: Accepted
- Date: 2026-10-26
- Amends: [ADR 0013](0013-phones-as-a-source-android-first.md) ("iOS only once a Mac and an Apple
  Developer account are available")

## Context
ADR 0013 put iOS (Phase 4b) after Android and made it wait for a Mac and a paid Apple Developer
account (99 USD a year). The author has neither a Mac nor the wish to pay, but has an iPhone to
test on, and wants iOS now.

Some things only a Mac can do: Xcode builds the app, the Go toolchain needs Xcode's clang and
iOS SDK for librclone, and an iPhone runs only signed apps. Other frameworks (Flutter, React
Native) don't change this, and moving to one would mean rewriting the Vue UI, so staying on Tauri
was not in question.

What does exist without a Mac or a fee:

- **GitHub-hosted macOS runners.** The repository is private, so they count against the free
  plan's minutes (a Mac minute uses up several of them). With the account's spending limit at
  $0 (the default), a run past them is refused, never billed.
- **The iOS simulator**, on that runner: enough to run test binaries and the app itself. Appetize
  shows a simulator in a browser on its free plan.
- **A free Apple ID** signs an app for the owner's own iPhone for 7 days. Open-source tools sign
  and install from Linux over USB (Impactor, Sideloader).

## Decision
- **Builds run on GitHub Actions' macOS runner** (`.github/workflows/ios.yml`), only on a push
  to the `ios` branch or by hand, so the free minutes go to the runs that are wanted. Each run
  builds the iPhone's `.ipa`; the simulator app only when asked for or when Appetize is set up.
- **`gen/apple` is not committed**: `tauri ios init` makes it on every run. What the project
  needs on iOS lives outside it: `tauri.ios.conf.json`, and the app's Swift plugin as a Swift
  package in `src-tauri/ios`, built and linked by `src-tauri/build.rs` (as `tauri-plugin` does for
  a plugin crate). Android's project stays committed: there the plugin is Kotlin inside it.
- **librclone is linked in as a static archive** (`build.sh ios`, Go `c-archive`), since Go has no
  `c-shared` for iOS. `transport-rclone`'s `librclone-linked` feature links it and
  `Librclone::open_linked` calls it; `crates/host` takes it when built with that feature. Same
  `RcCall`, same contract suite, which CI runs inside the simulator before it builds the app.
- **Tests, in order:** the transport contract in the simulator (every run); the app in the
  simulator (Appetize, when its token is set as a secret); the app on the author's iPhone, signed
  with their free Apple ID from Linux. No paid account: no TestFlight, no App Store.
- **Sources on iOS are folders from the Files app** (on the iPhone, iCloud Drive, a USB drive),
  still plain paths for `scanner`. iOS lets the app read one only while it holds the folder's
  security-scoped access, so the plugin opens it when picked, keeps a bookmark, and opens every
  bookmark again at launch, before the engine resumes Jobs left mid-way.
- **The Photos library is not a Source** in this phase. It has no paths (PhotoKit assets, not
  files), so it would be a new kind of Source with its own ADR.
- **Sending runs while the app is open.** The screen is kept on while Jobs send; on leaving the
  app the plugin asks for the short extra time iOS gives (about 30 s) to finish the current
  Batch. After that iOS suspends the app; the Job continues when it comes back, or resumes at the
  next launch. The iPhone's texts (`iphone` in `en.json`/`pt.json`) say so instead of Android's
  "you can leave the app".

## Consequences
- Every iOS change costs a CI run (about 20–40 Mac minutes; a run with the caches warm is
  shorter), and there is no debugger: logs come from the simulator, Appetize, or the iPhone
  through the signing tool. The free minutes allow a handful of runs a month.
- An app signed with a free Apple ID stops opening after 7 days; installing the same `.ipa`
  again renews it and keeps its data. iOS needs Developer Mode on to run it.
- The signing tool is a third party's, and it signs in with the Apple ID: a separate Apple ID made
  only for this keeps the author's own out of it.
- A store release, TestFlight, or a paid account's year-long signature would need the Apple
  Developer account, and its own decision.
- Picked folders' bookmarks are kept for good and all opened at launch; a list that grows past a
  few hundred, or a way to forget a folder, waits until it matters.
- Rejected: a Mac rented by the hour (24 h minimum lease, not free), Codemagic (one more service
  and its own free minutes, no gain over Actions), a macOS VM on the PC (against Apple's licence),
  a public repository for unlimited minutes (the author's call, still open), and moving to
  Flutter or React Native (they need the same Mac, and the UI would be rewritten).
