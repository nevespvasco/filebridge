# Runbook — the gates on the real servers and a real phone

The code for Phases 0, 1, 2 and 4a is done; what is left is running each gate against the real
Nextcloud server and a real Android phone ([09 — Roadmap](../09-roadmap.md)). This page is the
step-by-step for that: what to have ready, the exact commands, what to check, what to send back,
and how to clean up.

| § | Gate | Where | Closes |
|---|---|---|---|
| [0](#0-before-you-start) | — | PC | Setup shared by every section |
| [1](#1-phase-0-real-benchmark) | Phase 0: renaming during send is fast enough | PC → server, CLI | [ADR 0007](../adr/0007-rename-in-transit-strategy.md), [results template](../benchmarks/phase-0-real.md) |
| [2](#2-nextcloud-direct-mode-from-the-cli) | Phase 1 item: Nextcloud direct mode on the real server | PC → server, CLI | [ADR 0009](../adr/0009-nextcloud-on-the-ssh-host.md) |
| [3](#3-phase-1-gate-from-the-app) | Phase 1: three real folders without the terminal | PC → server, app | Phase 1 |
| [4](#4-phase-2-gate-on-the-real-servers) | Phase 2: zero-difference verification, no encoding warnings | PC → server, app | Phase 2 |
| [5](#5-phase-4a-gate-on-a-real-android-phone) | Phase 4a: 1,000+ photos from a phone | phone → server, app | Phase 4a |

The sections are independent except where a "Prerequisites" line says otherwise; § 1 and § 2
can run in either order.

**Names used below are placeholders.** Replace them with yours when you type the commands, and
never write the real ones into the repo (docs, fixtures, commits):

| Placeholder | Means |
|---|---|
| `192.0.2.10` | The machine you SSH into: the Proxmox node, or the Nextcloud LXC/VM itself |
| `192.0.2.20` | The Nextcloud LXC reached directly (only when you test both setups) |
| `root` | The SSH user on that machine |
| `user` | Your Nextcloud user id |
| `cloud.example.com` | The address you open Nextcloud at |
| `101` | The Nextcloud container's id on the Proxmox node |
| `/tank/nextcloud-data` | Nextcloud's data folder as the SSH host sees it (the "files go to" line in § 2 prints yours) |
| `/tank/filebridge-bench` | A scratch folder on the same disk, outside Nextcloud's data folder |

## 0. Before you start

**Build and paths.** From the repository root, in the terminal you'll use for § 1 and § 2:

```bash
export PATH="$HOME/.cargo/bin:$HOME/.local/bin:$PWD/target/release:$PATH"
```

```bash
cargo build --release -p filebridge-cli -p filebridge-service
```

```bash
filebridge --version && filebridge-service --version && rclone version
```

`rclone` should be the version the repo pins (`RCLONE_VERSION` in `sidecars/librclone/build.sh`, v1.75.2 today).

**A folder for what you send back:**

```bash
mkdir -p ~/fb-real-tests
```

**The server's host key, under the address you'll type.** filebridge never reads
`~/.ssh/config`: an alias like `pve` doesn't work, give the IP (or the full name), the user and
the port. Host keys are always checked against `~/.ssh/known_hosts`, and the CLI's
`profile add-…` commands never ask, so the key must already be there **under that same
address**. Add it with filebridge's own "trust" step (ADR 0015):

```bash
filebridge profile trust-host --host 192.0.2.10
```

It prints `… is already trusted` (nothing to do), or the key type and its `SHA256:`
fingerprint, and asks. Compare it with what the server prints for its own key (on the server's
console): `ssh-keygen -lf /etc/ssh/ssh_host_ed25519_key.pub`, and answer `y` only if they
match. A server whose key changed is refused, with how to check it. (Connecting once with
`ssh root@192.0.2.10` and answering `yes` does the same.)

On a port other than 22 add `--port 2222` here and to every command below (`--sftp-port 2222`
for `bench`).

**Signing in.** filebridge uses your SSH agent, or a key file **without a passphrase**
(`--key-file`). With a passphrase-protected key, load it into the agent instead:

```bash
ssh-add -l
```

It must list a key the server accepts (`ssh root@192.0.2.10 true` works without asking for a
password).

**The CLI and the app's service don't share a data folder at the same time.** Commands that
change something (`profile add-…`, `send`, `install-key`, `remove-access`, …) take the data
folder's lock and stop with "another filebridge engine is using …" while `filebridge-service` runs
on it. Read-only commands (`jobs`, `job`, `report`, `issues`, `profile list`, `profile test`)
work alongside it. For § 2, use a test data folder of its own, so the app can stay open:

```bash
export FILEBRIDGE_DATA_DIR="$HOME/filebridge-real-tests"
```

Only in that terminal. Before starting the app in § 3, `unset FILEBRIDGE_DATA_DIR`, or the app's
service would use the test folder too.

**What to send back.** The terminal output of every command (the `tee` lines below save it under
`~/fb-real-tests/`), the screenshots asked for, and the "Check" results. You may replace your
IPs, host names and user names before sending; whoever writes the results into the repo must use
the placeholders above.

## 1. Phase 0 real benchmark

Does writing each file straight to its corrected name (plan A) keep up with a plain
`sync/copy` on the real network and disk? Fills in
[benchmarks/phase-0-real.md](../benchmarks/phase-0-real.md) and decides
[ADR 0007](../adr/0007-rename-in-transit-strategy.md).

### Prerequisites

- § 0 done (release build, host key under the IP, agent or `--key-file`).
- The server is the one direct mode writes to: the Proxmox node (Nextcloud in an LXC), or the
  Nextcloud machine reached directly.
- A scratch folder **on the same disk as Nextcloud's data, but outside it**, so Nextcloud never
  indexes the bench files: `/tank/filebridge-bench` below. Find the disk with
  `ssh root@192.0.2.10 'df -hT /tank/nextcloud-data'`.
- Free space there: **3 × the subset's size per run** (each mode writes a full copy), and the
  bench deletes nothing.
- Two subsets of `Backup`, each one folder (the bench takes one `--source`), with no non-UTF-8
  names:
  - **A, small files:** 5,000–20,000 files, mostly under 1 MiB (documents, a code tree, photos).
  - **B, large files:** a few GB, mostly over 10 MiB (videos, archives, disk images).
  The source is only read.

### Steps

1. **Describe each subset** (once for A, once for B):

   ```bash
   SRC="$HOME/Backup/Fotos 2019"
   ```

   ```bash
   { echo "files: $(find "$SRC" -type f | wc -l)  dirs: $(find "$SRC" -type d | wc -l)"
     du -s --apparent-size --block-size=1M "$SRC"
     find "$SRC" -type f -printf '%s\n' | awk '{ if ($1 < 1048576) s++; else if ($1 < 104857600) m++; else l++ } END { printf "<1 MiB: %d, 1-100 MiB: %d, >=100 MiB: %d\n", s, m, l }'
     filebridge scan "$SRC"; } 2>&1 | tee ~/fb-bench/A-subset.txt
   ```

   `filebridge scan` must say `non-UTF-8 names (…): 0`; pick another folder otherwise.

2. **Describe the machines and the network** (once):

   ```bash
   { git rev-parse --short HEAD; uname -srm; nproc; free -h; lsblk -d -o NAME,ROTA,SIZE,MODEL
     ping -c 20 192.0.2.10 | tail -2
     ssh root@192.0.2.10 'nproc; free -h; lsblk -d -o NAME,ROTA,SIZE,MODEL; df -hT /tank/filebridge-bench 2>/dev/null || df -hT /tank; ssh -V'
   } 2>&1 | tee ~/fb-bench/setup.txt
   ```

   Add whether the PC is on cable, Wi-Fi or a VPN (Tailscale). With `iperf3` on both ends,
   `iperf3 -c 192.0.2.10 -t 10` gives the raw throughput.

3. **Warm the page cache**, so the first mode doesn't pay for reading the disk the others get
   from memory:

   ```bash
   tar -cf - "$SRC" > /dev/null
   ```

4. **Run the bench:**

   ```bash
   filebridge bench --source "$SRC" --dest-root /tank/filebridge-bench \
     --sftp-host 192.0.2.10 --sftp-port 22 --sftp-user root \
     --modes baseline,plan-a,engine --concurrency 8 \
     --json ~/fb-bench/A-c8.json 2>&1 | tee ~/fb-bench/A-c8.log
   ```

   - `--sftp-user` is required in practice: without it rclone signs in as your local user.
   - Add `--key-file ~/.ssh/id_ed25519` when not using the agent. rclone is found like in the
     other commands (`--rclone`, else `rclone` on `PATH`, else `~/.local/bin/rclone`).
   - It prints the source (`source: N files, …; nfc corrects …`), then
     `rclone v…; writing under /tank/filebridge-bench/filebridge-bench-<unix time>`, a progress
     line per mode, a table, and last `bench folders were left at the destination under …`.
     **Write down that folder**: it is what you delete in Cleanup.
   - It takes about three copies' time. Ctrl-C stops it; the JSON is written at the end (its
     folder, `~/fb-bench`, is created first).

5. **Repeat for subset B** with `--json ~/fb-bench/B-c8.json … | tee ~/fb-bench/B-c8.log`
   (warm the cache again first).

6. **Optional**, if there's time and space: subset A with `--concurrency 32` (`A-c32`), and with
   the modes in reverse order, `--modes engine,plan-a,baseline` (`A-c8-reverse`), to see whether
   the order matters.

### Check

- In every row of every table: `failed`, `missing`, `unexpected` and `skipped` are 0. The
  `missing:` / `unexpected:` lines under a mode show up to three examples when they aren't;
  `skipped` (engine only) counts files left out because their name needed a decision, which
  are then not in its files/s.
- The `files` column is the same in the three rows.
- `engine` files/s ÷ `baseline` files/s (subset A) and `engine` MB/s ÷ `baseline` MB/s
  (subset B) are about **0.8 or more**; the same for `plan-a`. That is the ADR 0007 rule.

### Send back

`~/fb-bench/` as it is: `setup.txt`, the `*-subset.txt` files, and the `.log` and `.json` of every
run. The JSON holds only counts and timings (no host or path), so it can go into
`docs/benchmarks/phase-0-real/` unchanged.

### Cleanup

The bench never deletes anything at the destination. On the server, look first, then delete
**only** the `filebridge-bench-<unix time>` folders the runs printed:

```bash
ssh root@192.0.2.10 'du -sh /tank/filebridge-bench/filebridge-bench-*'
```

```bash
ssh root@192.0.2.10 'rm -rf /tank/filebridge-bench/filebridge-bench-1792745028'
```

(one per run; the number is the one printed). On the PC nothing is left: the `engine` mode's
database lives in a temporary folder removed at the end. Keep `~/fb-bench/` until the results are
recorded.

## 2. Nextcloud direct mode from the CLI

Direct mode on the real server: detection (through the Proxmox node and `pct`, or SSH straight
into the machine Nextcloud runs on, [ADR 0009](../adr/0009-nextcloud-on-the-ssh-host.md)), owner
`100033`, the scan of only the sent paths, and filebridge's own key
([ADR 0012](../adr/0012-own-ssh-key-with-restrict.md)). Closes the Phase 1 item "Nextcloud direct
mode … not yet run against the real server".

### Prerequisites

- § 0 done, and `FILEBRIDGE_DATA_DIR="$HOME/filebridge-real-tests"` exported in this terminal.
- **SSH as `root`.** The files are written over SFTP *as the SSH user*, so that user itself must
  be able to write in `<data>/user/files`, which belongs to the web server's user; sudo alone
  isn't enough. Detection checks it in its `writable` step. Use `root`.
- Nextcloud user `user` exists and has signed in once (its `files` folder exists).
- Which setup you test, and its host:
  - **Proxmox node** (`192.0.2.10`): Nextcloud in an LXC; commands go through `pct exec`.
  - **Nextcloud machine itself** (`192.0.2.20`): its LXC/VM reached directly, e.g. over
    Tailscale. Test this too if that's how you reach it day to day.
- A test source. A synthetic one with names to correct (30 % of the accented names in NFD):

  ```bash
  filebridge fixture ~/fb-nc-fixture --files 300 --size 65536 --dirs 6 --nfd-percent 30
  ```

  Plus, if you like, a small real folder (a few hundred files with accents in their names).

### Steps

1. **Detect and save the profile:**

   ```bash
   filebridge profile add-nextcloud --name nc-direct --host 192.0.2.10 --user root \
     --nc-user user --folder filebridge-test --verbose 2>&1 | tee ~/fb-real-tests/2-1-detect.txt
   ```

   - `--verbose` prints every command detection ran (never their output: `config.php` holds the
     database password, so only its `datadirectory` line is read).
   - Several running containers named like Nextcloud: it stops with
     `containers found: …` / `pick one with --ctid`. Add `--ctid 101`.
   - `--ctid` only on a Proxmox node: on the Nextcloud machine itself it fails with "`pct` not
     found, so this is not a Proxmox node".
   - Nextcloud not in `/var/www/nextcloud`, `/var/www/html/nextcloud`, `/var/www/html`,
     `/usr/share/webapps/nextcloud`, `/srv/nextcloud` or `/opt/nextcloud`: add
     `--install-dir /path/to/nextcloud`.
   - Also available: `--port`, `--key-file`, `--known-hosts`, `--dir-mode` (default `750`),
     `--file-mode` (default `640`).

   Expected, on the **Proxmox node**: `✓` for `pct-list` (container 101 (…) is running),
   `pct-config`, `datadirectory` (`… in the container is … on the host`), `data-path`,
   `writable` (`root can write to …`), `occ`,
   `run-as` (`Nextcloud <version> answers through sudo` or `su (no sudo in the container)`) and
   `owner` (`files will be owned by 100033:100033 (www-data 33:33 in the container)`); then
   `Nextcloud runs in a container on the Proxmox node`, `files go to /…/user/files/filebridge-test`
   and `saved profile "nc-direct"`.

   On the **Nextcloud machine itself**: `pct-list` is `✓` with "no `pct` here, so this is not a
   Proxmox node: looking for Nextcloud on this machine itself", then `host` (`commands run as
   root`), `datadirectory`, `data-path`, `writable`, `occ`, `run-as`, `owner` (the web user's own ids, e.g.
   `33:33`), and `Nextcloud runs on the SSH host itself`.

   A `!` on `owner` means the data folder belongs to someone else than the ids it computed:
   send that output back before going on.

2. **Connection test:**

   ```bash
   filebridge profile test nc-direct 2>&1 | tee ~/fb-real-tests/2-2-test.txt
   ```

   The transport's checks, then Nextcloud's: `ssh`, `data-path`, `writable`, `occ`
   (`Nextcloud <version>`), `free-space`, all `✓`.

3. **Send with verification** (signing in with your agent or key file):

   ```bash
   filebridge send ~/fb-nc-fixture --to nc-direct --verify 2>&1 | tee ~/fb-real-tests/2-3-send.txt
   ```

   It prints the analysis (the `nfc` names are "fixed automatically"), the checks, and asks
   `Send now? [y/N]`. Then progress, `nextcloud.scan: …/… folders, … items`, and
   `✓ done: 300 files, …`. The folder lands as `filebridge-test/fb-nc-fixture` (a sent folder
   keeps its name). If it stops at "N names need a decision", follow the commands it
   prints (`filebridge issues …`, `filebridge decide-all … accept`, `filebridge start …`).

   ```bash
   filebridge jobs --limit 3
   ```

   ```bash
   filebridge job <first 8 characters of the job id> | tee ~/fb-real-tests/2-3-job.txt
   ```

   Expect `state completed` and `verified 300 ok, 0 differ`.

4. **Owner and modes at the destination.** Use the path the "files go to" line printed:

   ```bash
   ssh root@192.0.2.10 'ls -lnd /tank/nextcloud-data/user/files/filebridge-test; ls -lnR /tank/nextcloud-data/user/files/filebridge-test | head -20' \
     | tee ~/fb-real-tests/2-4-ls.txt
   ```

   On the Proxmox node, the owner and group columns are `100033 100033` everywhere, folders
   `drwxr-x---` and files `-rw-r-----`. On the Nextcloud machine itself, the web user's ids
   (e.g. `33 33`). Optionally, inside the container: `pct exec 101 -- ls -ln <data dir in the
   container>/user/files/filebridge-test` shows `33 33`.

5. **Nextcloud sees the files, with no manual scan.** Don't run `occ files:scan`. Open
   `https://cloud.example.com` → Files → `filebridge-test/fb-nc-fixture`: every folder and file is there, under
   its corrected name; open two or three. Take a screenshot.

6. **Install filebridge's own key:**

   ```bash
   filebridge profile install-key nc-direct 2>&1 | tee ~/fb-real-tests/2-6-install-key.txt
   ```

   Answer `y`. Steps `generate`, `install` (`added to root@192.0.2.10's authorized_keys,
   restricted`), `keyring`, `sign-in`, `transfer`, `saved`, all `✓`. Then:

   ```bash
   ssh root@192.0.2.10 'grep -n filebridge- ~/.ssh/authorized_keys'
   ```

   One line: `restrict ssh-ed25519 AAAA… filebridge-<profile id>`.

   Prove the profile no longer needs your agent: run the next two commands with the agent hidden
   (`env -u SSH_AUTH_SOCK` removes it for that one command only).

   ```bash
   env -u SSH_AUTH_SOCK filebridge profile test nc-direct
   ```

   ```bash
   env -u SSH_AUTH_SOCK filebridge send ~/fb-nc-fixture --to nc-direct --into with-own-key --verify 2>&1 \
     | tee ~/fb-real-tests/2-6-send.txt
   ```

   Both work; the second lands in `filebridge-test/with-own-key/fb-nc-fixture`, also owned by
   `100033`.

7. **Remove access:**

   ```bash
   filebridge profile remove-access nc-direct 2>&1 | tee ~/fb-real-tests/2-7-remove-access.txt
   ```

   Answer `y`. Steps `remove`, `proof` (a last sign-in with the removed key is refused),
   `saved` (`the destination signs in with your key file or SSH agent again`) and `keyring`,
   all `✓`. Then:

   ```bash
   ssh root@192.0.2.10 'grep -c filebridge- ~/.ssh/authorized_keys'
   ```

   prints `0`, and `filebridge profile test nc-direct` works again (with the agent).

8. **Optional, the other setup:** repeat steps 1–7 with `--name nc-direct-lxc --host
   192.0.2.20 --folder filebridge-test-2` (paths in step 4 as its "files go to" line says).

### Send back

`~/fb-real-tests/2-*.txt`, the screenshot from step 5, and which setup(s) you ran.

### Cleanup

- In Nextcloud's web UI, delete `filebridge-test` (and `filebridge-test-2`), then empty them from
  "Deleted files" if you want the space back.
- Forget the profile: `filebridge profile remove nc-direct` (and `nc-direct-lxc`).
- Once the results are recorded: `rm -rf ~/fb-nc-fixture` (the synthetic source) and
  `rm -rf ~/filebridge-real-tests` (the test data folder: these profiles and jobs only).
- `unset FILEBRIDGE_DATA_DIR`.

## 3. Phase 1 gate from the app

**Gate:** send three real folders (a large work folder, a code repository, and one with a comma
in its name) without opening the terminal. The terminal only starts the app; everything from
"Add the destination" on happens in the window.

### Prerequisites

- § 0 done; the app built from the same commit as the service.
- `unset FILEBRIDGE_DATA_DIR` in the terminal that starts the app.
- `pnpm install` run once in `apps/desktop`.
- The three folders, on the PC:
  - a **large work folder** (tens of GB, the real one);
  - a **code repository**, `.git` included (many small files; symbolic links are not sent and are
    listed in the analysis);
  - a folder **with a comma in its name**, e.g. `Contas, 2024`. If none exists, copy one to a
    new name; never rename the original.
- Room at the destination for all three.

### Start the app

Two ways. The second is better for the large folder: the service is a release build, and it
survives the window and the terminal.

- **A, app only:**

  ```bash
  cd apps/desktop && pnpm tauri dev
  ```

  It builds `filebridge-service` (debug) first; the window starts the service when none is
  running, and the service keeps sending when the window closes.

- **B, service as a `systemd --user` unit, then the app:**

  ```bash
  filebridge-service install-user-unit
  ```

  (the `target/release` binary, from `PATH`). It prints `installed
  ~/.config/systemd/user/filebridge-service.service` and starts it. Check:

  ```bash
  filebridge-service status
  ```

  `running: version …, protocol …`. The unit's environment must reach your SSH agent, or every
  SSH sign-in with it fails; `install-user-unit` warns (`! the service can't reach your SSH
  agent …`) when it doesn't. To check by hand:

  ```bash
  systemctl --user show-environment | grep SSH_AUTH_SOCK
  ```

  No output: `systemctl --user import-environment SSH_AUTH_SOCK` and
  `systemctl --user restart filebridge-service`. Then start the window with
  `cd apps/desktop && pnpm tauri dev`; it connects to the running service.

### Steps (in the app)

1. **Add the destination.** Destinations → add → Nextcloud → **Directly on the server (SSH)**:
   server `192.0.2.10`, SSH user `root`, Nextcloud user `user`, a folder (e.g. `Arquivo`).
   Detect. If the host isn't in `known_hosts`, **Trust this server?** shows its key type and
   fingerprint: compare with `ssh-keygen -lf /etc/ssh/ssh_host_ed25519_key.pub` on the server,
   then "Trust and continue". Every step `✓` in Details; save. **Connection test** `✓`.
   (Plain SFTP to a folder outside Nextcloud also passes this gate, if you prefer.)
2. **Send each folder.** New transfer → **Source**: the folder → **Destination**: the profile →
   **Analysis**: look at the names it will correct, decide any it asks about → **Review & send**:
   tick **Verify every file at the end** → send.
   - During the large one, close the window and open it again (`pnpm tauri dev`, or the tray):
     the transfer is still running. Optional: Pause, then Resume.
   - The three can be queued one after another.
3. **Check each one** on its transfer page: finished, the file count matches the analysis, and
   `Verified N files: 0 different, 0 missing, 0 couldn't be checked`. In Nextcloud's web UI, the
   three folders are there; the comma folder keeps its name (`Contas, 2024`).

### Check

- All three finished without opening a terminal after the app started, each verification with
  0 different and 0 missing.
- Any warning (skipped symbolic links, names left as they were, a Nextcloud problem) is listed;
  note it.

### Send back

- For each folder: files, size, duration, final state, the verification line, and a screenshot of
  its transfer page.
- The engine's own record, from a terminal **after** the three are done (read-only, fine with the
  service running):

  ```bash
  filebridge jobs --limit 5 | tee ~/fb-real-tests/3-jobs.txt
  ```

  ```bash
  filebridge job <id> | tee -a ~/fb-real-tests/3-jobs.txt
  ```

  (once per job), and with option B the service's warnings:
  `journalctl --user -u filebridge-service --since today | grep -i -E 'warn|error' > ~/fb-real-tests/3-service.txt`.

### Cleanup

Nothing to undo: these are real copies; keep them. The destination stays for § 4. To drop the
unit later: `filebridge-service uninstall-user-unit` (running transfers resume at the next
start).

## 4. Phase 2 gate on the real servers

**Gate:** a verification with no differences and a scan with no encoding warnings. Covers WebDAV
with an app password in the keyring, SSH with filebridge's own key, and "Repair destination".

### Prerequisites

- The app running as in § 3, with the direct-mode destination from § 3.
- An **app password** for filebridge: Nextcloud → Personal settings → Security → Devices &
  sessions → name `filebridge-test` → "Create new app password". Copy it (it's shown once). Not
  the account password: that one is about 4× slower per request
  ([05](../05-integration-nextcloud.md#as-built-phase-2)).
- Two real folders not sent before: one of a few hundred to ~1,000 files for WebDAV (verifying
  over WebDAV downloads every file, about one file a second), one bigger for direct mode.

### Steps

1. **WebDAV with the app password in the keyring.** Destinations → add → Nextcloud → **Over the
   internet (WebDAV)**: address `cloud.example.com`, user `user`, the app password, folder
   `filebridge-webdav-test`. Save: the `address`, `sign-in` and `space` steps `✓`, and `folder`
   `✓` (or `!` "doesn't exist yet: the first send creates it").
   - The password is in the keyring: in GNOME's "Passwords and Keys" (Seahorse), Login, an entry
     named `filebridge/<profile id>/app-password` (KDE: KWalletManager).
   - It is nowhere on disk or in the logs. In a terminal (the password is read without echo and
     not kept in the shell history):

     ```bash
     printf 'App password: '; read -rs P; echo
     ```

     ```bash
     grep -r -a -l -F -e "$P" ~/.local/share/filebridge ~/.config/filebridge 2>/dev/null; journalctl --user -u filebridge-service | grep -c -F -e "$P"; unset P
     ```

     Expect no file names, then `0` (the `journalctl` part only means something with option B
     of § 3).
   - New transfer of the WebDAV folder to this destination, **Verify every file at the end**
     ticked: `0 different, 0 missing`.
2. **SSH with filebridge's own key.** Destinations → the direct-mode destination → **Use
   filebridge's own key…** → "Install the key". "Done: this destination now uses filebridge's own
   key." Then:

   ```bash
   ssh root@192.0.2.10 'grep -n filebridge- ~/.ssh/authorized_keys'
   ```

   One `restrict ssh-ed25519 … filebridge-<profile id>` line; in the keyring, an entry
   `filebridge/<profile id>/ssh-key`. **Connection test** `✓`.
3. **Zero-difference verification and a clean scan.** New transfer of the bigger folder to the
   direct-mode destination, verification ticked. On its transfer page:
   - `Verified N files: 0 different, 0 missing, 0 couldn't be checked`;
   - Nextcloud's indexing step finished, and **no** "will not be accessible due to incompatible
     encoding" problem is listed.

   Optional cross-check on the server, scanning only that folder (use the `run-as` way § 2 or
   the Details panel showed; with `sudo` in the container it is `sudo -u www-data php …`):

   ```bash
   ssh root@192.0.2.10 "pct exec 101 -- su -s /bin/sh www-data -c 'php /var/www/nextcloud/occ files:scan --path=\"user/files/Arquivo/<folder>\"'"
   ```

   No `incompatible encoding` line in its output. (On the Nextcloud machine itself, drop
   `pct exec 101 --`.)
4. **Repair destination.** Make three badly named files in a new folder **inside the
   destination's folder** on the server: an NFD name, a Latin-1 name and a mojibake name. On the
   Proxmox node (owner `100033`; on the Nextcloud machine itself, the ids from the `owner` step):

   ```bash
   ssh root@192.0.2.10 'D=/tank/nextcloud-data/user/files/Arquivo/fb-repair-test; mkdir "$D" && cd "$D" && touch "$(printf "Vera\314\203o.txt")" "$(printf "caf\351.txt")" "$(printf "Ol\303\203\302\241.txt")" && chown -R 100033:100033 "$D" && ls -ln "$D"'
   ```

   (`Verão` in NFD, `café` in Latin-1, and `OlÃ¡`, the mojibake of `Olá`.) Then in the app:
   Destinations → the direct-mode destination → **Repair names…** → pick `fb-repair-test` →
   review the proposed names (expected: `Verão.txt`, `café.txt`, `Olá.txt`; note any that
   differ), accept or edit → rename.
   Check:
   - the repair finished, with Nextcloud's indexing and no encoding problem listed;
   - in Nextcloud's web UI, `fb-repair-test` shows the three corrected names;
   - `ssh root@192.0.2.10 'ls /tank/nextcloud-data/user/files/Arquivo/fb-repair-test'` shows
     the same names.
5. **Remove access.** Destinations → the direct-mode destination → **Remove access…** → confirm.
   Then `ssh root@192.0.2.10 'grep -c filebridge- ~/.ssh/authorized_keys'` prints `0`, the
   `ssh-key` entry is gone from the keyring, and **Connection test** works again with your agent
   or key file (with option B of § 3, only if the unit sees `SSH_AUTH_SOCK`).

### Check

- WebDAV: password only in the keyring; verification 0 different, 0 missing.
- Direct mode with filebridge's own key: verification 0 different, 0 missing; no encoding
  warning from the scan.
- Repair: the three names fixed at the destination and visible in Nextcloud.
- Remove access: the line and the keyring entry gone.

### Send back

Screenshots of: the WebDAV save steps, both transfer pages (verification line and problems
list), the repair's review and its finished page, and the remove-access result. The output of
the `grep`/`ls` commands above, and `filebridge job <id>` for each of the three jobs (send,
send, repair).

### Cleanup

- In Nextcloud's web UI, delete `filebridge-webdav-test` and `Arquivo/fb-repair-test` if you
  don't want them, and empty them from "Deleted files".
- If you won't keep the WebDAV destination: delete it in Destinations, then revoke the
  `filebridge-test` app password in Nextcloud (Devices & sessions).

## 5. Phase 4a gate on a real Android phone

**Gate:** from an Android phone, send a real folder with at least 1,000 photos and videos to
Nextcloud over WebDAV; turn the screen off and switch apps mid-send; force-stop the app once and
see the transfer resume; the verification has zero differences. Then the part not yet tried on
any phone: an SSH destination, with "Trust this server?" and a key file
([11 — Mobile](../11-mobile.md), [ADR 0015](../adr/0015-trust-unknown-host-key-in-the-app.md)).

### Prerequisites

- An arm64 Android phone, Android 8 or later, with USB debugging on (Settings → About → tap
  "Build number" 7 times; then Developer options → USB debugging). `adb devices` lists it.
- The phone reaches Nextcloud (same Wi-Fi, or Tailscale on the phone).
- The APK, built as in `CLAUDE.md` / [11 — Mobile](../11-mobile.md):

  ```bash
  sidecars/librclone/build.sh android
  ```

  ```bash
  cd apps/desktop && pnpm tauri android build --apk --target aarch64
  ```

  with `ANDROID_HOME`, `NDK_HOME` and `JAVA_HOME` (JDK 17 or 21) set. The APK lands under
  `apps/desktop/src-tauri/gen/android/app/build/outputs/apk/universal/release/`. It must be
  signed (`adb install` refuses an `…-unsigned.apk`); see docs/11 for the key. An APK signed with
  another key than the installed one can't update it: uninstalling first deletes the app's
  data.
- A new app password for the phone in Nextcloud, e.g. `filebridge-phone`.
- A folder on the phone with **at least 1,000 photos and videos**, e.g. `DCIM/Camera`. Count it:

  ```bash
  adb shell 'ls /sdcard/DCIM/Camera | wc -l; du -sh /sdcard/DCIM/Camera'
  ```

### Steps

1. **Install and open:**

   ```bash
   adb install -r apps/desktop/src-tauri/gen/android/app/build/outputs/apk/universal/release/app-universal-release.apk
   ```

   Open filebridge. When asked, allow **All files access** (Android's settings page opens; turn it
   on and go back).
2. **WebDAV destination:** Destinations → add → Nextcloud → **Over the internet (WebDAV)**:
   `cloud.example.com`, `user`, the `filebridge-phone` app password, folder `phone-gate`. Save.
3. **Start the transfer:** New transfer → Source: the photo folder → Destination: `phone-gate`
   → Analysis → Review & send: tick **Verify every file at the end** → send. At the first send
   the app asks to be exempted from battery optimisation: allow it. Note the time.
4. **Mid-send, in this order**, noting the time and the file count each time:
   1. Screen off (power button) for at least 2 minutes, then back on: the count went up; the
      notification is there.
   2. Switch to another app (camera, browser) for at least a minute, then back.
   3. At about 30–60 %, **force-stop**: Settings → Apps → filebridge → Force stop (or
      `adb shell am force-stop <package>`; the id is `identifier` in
      `apps/desktop/src-tauri/tauri.android.conf.json`, or
      `adb shell pm list packages | grep filebridge`). The notification disappears.
   4. Open filebridge again: the transfer resumes by itself from where it stopped (not from 0).
      Note whether it resumed by itself or needed "Resume".
5. **Verification:** after the copy, it checks every file by downloading it again (about one
   file a second, so 1,000 files ≈ 20 minutes); the app can stay in the background. Expect
   `Verified N files: 0 different, 0 missing, 0 couldn't be checked`.
6. **In Nextcloud's web UI**, `phone-gate` holds as many files as the source; open a photo and a
   video.

### Then: an SSH destination on the phone

A phone has no SSH agent, so the first sign-in uses a key file; then filebridge's own key takes
over (kept in the Android Keystore) and the file is deleted.

1. **A temporary key**, on the PC (no passphrase: filebridge can't open a key file that has one):

   ```bash
   ssh-keygen -t ed25519 -N '' -C fb-phone-test -f ~/fb-phone-test-key
   ```

   ```bash
   ssh-copy-id -i ~/fb-phone-test-key.pub root@192.0.2.10
   ```

   ```bash
   adb push ~/fb-phone-test-key /sdcard/Download/fb-phone-test-key
   ```

   And the fingerprint to compare with, from your PC's `known_hosts`:
   `ssh-keygen -l -F 192.0.2.10`.
2. **The destination:** Destinations → add → SFTP: host `192.0.2.10`, port 22, user `root`,
   Private key `/storage/emulated/0/Download/fb-phone-test-key` (the full path: `~` is the
   app's own folder on a phone), folder `/tank/filebridge-phone-test` (outside Nextcloud). Save.
   **Trust this server?** appears: the key type and `SHA256:` fingerprint must match step 1's;
   then "Trust and continue". **Connection test** `✓`. Screenshot the dialog.
3. **Switch to filebridge's own key:** **Use filebridge's own key…** → "Install the key". Then
   delete the key file and its temporary line:

   ```bash
   adb shell rm /sdcard/Download/fb-phone-test-key
   ```

   ```bash
   ssh root@192.0.2.10 'grep -n -e fb-phone-test -e filebridge- ~/.ssh/authorized_keys'
   ```

   ```bash
   ssh root@192.0.2.10 "sed -i.bak '/ fb-phone-test\$/d' ~/.ssh/authorized_keys && grep -c fb-phone-test ~/.ssh/authorized_keys"
   ```

   The last command keeps a copy as `authorized_keys.bak` and prints `0`. The `filebridge-…`
   line stays. **Connection test** on the phone still `✓` (now with its own key).
4. **Send a small folder** (a few dozen photos) to it with verification: `0 different`.
5. **Remove access** on the phone: the `filebridge-` line is gone
   (`ssh root@192.0.2.10 'grep -c filebridge- ~/.ssh/authorized_keys'` prints `0`).
6. Optional: the same with Nextcloud → **Directly on the server (SSH)** on the phone.

### Check

- WebDAV: ≥ 1,000 files sent; screen off, app switch and force-stop survived; resumed without
  starting over; verification 0 different, 0 missing; the counts match in Nextcloud.
- SSH: "Trust this server?" showed the right fingerprint; key file, then own key, then remove
  access all worked.

### Send back

- Phone model, Android version, the commit the APK was built from.
- Source count and size; the times and file counts of: start, screen off/on, app switch,
  force-stop, reopen, end of copy, end of verification; whether the resume was automatic.
- Screenshots: the transfer page during the send, after the reopen, and at the end (verification
  line); the "Trust this server?" dialog; the SSH test send's page.
- Optionally the log, filtered to the app (read it before sending: it may hold file names):
  `adb logcat -d -v time | grep -i -E 'filebridge|rclone|RustStdoutStderr' > ~/fb-real-tests/5-logcat.txt`.

### Cleanup

- Server: look, then delete the SFTP test folder:
  `ssh root@192.0.2.10 'du -sh /tank/filebridge-phone-test'`, then
  `ssh root@192.0.2.10 'rm -rf /tank/filebridge-phone-test'`; once all is well,
  `ssh root@192.0.2.10 'rm ~/.ssh/authorized_keys.bak'`.
- PC: `rm ~/fb-phone-test-key ~/fb-phone-test-key.pub`.
- Nextcloud: `phone-gate` holds real photos; keep it or delete it. Revoke the `filebridge-phone`
  app password if you won't keep the phone's destination.
- Phone: delete the test destinations in the app if you won't use them.
