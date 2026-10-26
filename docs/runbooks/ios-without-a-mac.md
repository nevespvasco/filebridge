# Runbook — iOS without a Mac

How to build filebridge for iOS on GitHub's Mac, try it in a simulator in the browser, and install
it on your own iPhone with a free Apple ID, all from Linux and at no cost
([ADR 0034](../adr/0034-ios-without-a-mac.md)). The Phase 4b gate is at the end.

## 0. Once

1. **GitHub's spending limit stays at $0** (Settings → Billing → Budgets, the default for a
   monthly plan). A run past the free minutes is then refused, never billed.
2. **Appetize (optional, free plan):** make an account at appetize.io, create an API token, and
   save it in the repository as the secret `APPETIZE_API_TOKEN` (Settings → Secrets and
   variables → Actions). After the first upload, save the `publicKey` the run prints as the
   repository variable `APPETIZE_APP_KEY`, so later runs replace the same app page instead of
   making a new one.
3. **An Apple ID** to sign with. A separate one made only for this keeps your own out of a third
   party's tool (it signs in with it).
4. **A signing tool on Linux:** [Impactor](https://github.com/khcrysalis/Impactor) (open source;
   [Sideloader](https://github.com/Dadoum/Sideloader) is an alternative), and `usbmuxd`, which
   lets Linux talk to an iPhone over USB (`sudo apt install usbmuxd` on Debian/Ubuntu).
5. **On the iPhone:** connect it by USB and tap "Trust this computer". Later, after the first
   install: Settings → Privacy & Security → **Developer Mode** on (the iPhone restarts), and
   Settings → General → VPN & Device Management → trust your Apple ID's profile.

## 1. Build

A push to the `ios` branch starts `.github/workflows/ios.yml`. Once the workflow is on `main`, it
can also be started by hand (GitHub only offers "Run workflow" for workflows on the default
branch):

```bash
gh workflow run ios.yml --ref ios -f simulator=true
```

`simulator=true` also builds the app for the simulator (always, once the Appetize secret is set);
without it only the iPhone's `.ipa` is built, which uses fewer minutes. Follow the run with
`gh run watch`.

What a green run proves, step by step:

| Step | Shows |
|---|---|
| Build librclone | rclone cross-compiles for iOS (cached afterwards) |
| Transport contract in the simulator | librclone linked into a program works on iOS: copy, move, probe |
| Unsigned app for a real iPhone | the app builds and links for a device: Rust, librclone, the Swift plugin |
| App for the simulator | the same for the simulator (with `simulator=true` or Appetize) |

Then download the artifacts:

```bash
gh run download --name filebridge-ios-device
```

## 2. Try it in the simulator (Appetize)

With the secret set, the run's "Upload to Appetize" step prints the app's page (`appURL`). Open it
in the browser: an iPhone appears and runs filebridge. Check that the app opens, the screens fit,
and a destination can be added. The simulator has the Files app with "On My iPhone", but no photos
of yours; don't judge speed there.

## 3. Install on your iPhone

1. Connect the iPhone by USB.
2. Open the signing tool, sign in with the Apple ID, pick `filebridge-ios-device.ipa`, install.
3. On the iPhone, trust the profile and turn Developer Mode on (§ 0.5) if it asks.

The app opens for **7 days**. After that its icon stays but it closes at once: install the same
`.ipa` again (step 2), and it opens for another 7 days with its data kept.

## 4. Phase 4b gate

1. Copy a test folder of a few hundred photos and videos into the Files app (On My iPhone, or
   iCloud Drive, downloaded).
2. In filebridge, add a Nextcloud destination (WebDAV, app password), pick that folder, send.
3. Mid-send, go to the home screen for a minute, then come back: the send continues.
4. Mid-send again, close the app from the app switcher, open it: the Job resumes by itself.
5. At the end: verification with **zero differences**.

Send back: the run's link, the Job's Report, and what the screen showed at steps 3 and 4. Logs:
the signing tool shows the iPhone's console, or `idevicesyslog | grep filebridge` (from
`libimobiledevice-utils`).

## Known gaps

- The Photos library is not a Source yet: only folders from the Files app.
- Every iOS change needs a CI run; there is no local build or debugger.
- Picked folders are remembered for good (no way yet to forget one).
