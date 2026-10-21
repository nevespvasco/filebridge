# Phase 0 benchmark — simulated SFTP (2026-10-21)

Can rclone write every file under its corrected (NFC) name fast enough? This page compares three
ways to send the same tree over SFTP:

| Mode | What it does |
|---|---|
| **baseline** | Plain `sync/copy` with the original names: what rclone/rsync does today. |
| **plan A** | One `operations/copyfile` per file, straight to the NFC name. |
| **plan B** | `sync/copy` to a temp folder next to the destination with the original names, batch rename there (`operations/movefile` for files, `sync/move` for directories), then `sync/move` into place. |

> **Not the real server yet.** These numbers come from a local test server with simulated
> latency. The run against the real Nextcloud server and the real `Backup` folder is still
> pending. See [ADR 0007](../adr/0007-rename-in-transit-strategy.md).

## Setup

- Machine: one Linux x86_64 host (32 threads). Client, test server and latency proxy all run on it.
- rclone v1.75.2 (`rclone rcd`, driven by `filebridge bench`), same `--transfers`/concurrency
  for every mode.
- Server: `rclone serve sftp --no-unicode-normalization`, which compares names byte for byte
  like OpenSSH on ext4.
- Latency: [`tools/bench/delay_proxy.py`](../../tools/bench/delay_proxy.py), a userspace TCP
  proxy that delays each direction by half the RTT. It adds no bandwidth cap and no loss, but
  it has its own per-chunk overhead.
- Fixtures (`filebridge fixture`): random content, 30 % of the accented names written in NFD,
  and some NFD directory names, so every child of those directories also changes path.
  - *small*: 2,000 × 16 KiB in 20 dirs (535 file + 6 dir names to correct, 957 file paths
    change). *small500*: 500 × 16 KiB in 10 dirs (138 nodes to rename).
  - *big*: 40 × 8 MiB in 4 dirs.
- After every run the destination is listed and compared with the expected paths. **Every run
  below had 0 failed, 0 missing and 0 unexpected paths.**
- Raw results: [phase-0-simulated/](phase-0-simulated/). Each figure is a single run.

## Results

| Fixture | RTT | Concurrency | baseline | plan A | plan B | A / baseline | B / baseline |
|---|---|---|---|---|---|---|---|
| small (2,000) | ~0 | 8 | 1,099 files/s | 2,533 files/s | 901 files/s | 2.30 | 0.82 |
| small (2,000) | 2 ms | 8 | 187 files/s | 176 files/s | 144 files/s | 0.94 | 0.77 |
| small500 | 20 ms | 8 | 24.1 files/s | 23.2 files/s | 17.2 files/s | 0.96 | 0.71 |
| small500 | 20 ms | 32 | 33.9 files/s | 35.2 files/s | 22.7 files/s | 1.04 | 0.67 |
| big (40 × 8 MiB) | ~0 | 8 | 415 MB/s | 459 MB/s | 274 MB/s | 1.11 | 0.66 |
| big (40 × 8 MiB) | 20 ms | 8 | 92 MB/s | 110 MB/s | 63 MB/s | 1.19 | 0.68 |

Plan B stage breakdown (seconds):

| Run | copy | rename | move into place |
|---|---|---|---|
| small, ~0 RTT | 1.8 | 0.2 | 0.2 |
| small, 2 ms | 10.3 | 3.4 | 0.2 |
| small500, 20 ms, c8 | 20.4 | 7.9 | 0.8 |
| small500, 20 ms, c32 | 15.7 | 5.7 | 0.6 |
| big, 20 ms | 3.4 | 1.3 | 0.6 |

### Reading

- **Plan A is as fast as the baseline once there is any latency (0.94–1.19×).** With no
  latency it was faster still. rclone already uploads every file to a `.partial` name and renames
  it on completion, so writing to the NFC name adds no round trip. What plan A gives up is the
  batched directory listing of `sync/copy`, and with latency that cost doesn't show.
- **Plan B is always slower (0.66–0.82×).** It pays the full copy, then 2 server-side renames per
  renamed node (see the hazard below), then the move into place. The rename stage grows with the
  number of renamed nodes × RTT.
- More concurrency helps all modes over high latency (c8 → c32: +40 % baseline, +52 % plan A).
- The 2 ms run is slower than real 2 ms networks would be: the Python proxy's own overhead
  dominates there. The plans are compared against each other under the same conditions, not on
  absolute numbers.

## Hazard found: plan B can delete files on a normalising destination

Some filesystems treat NFD and NFC spellings as the **same** name (macOS APFS, some SMB/ZFS
setups, rclone's own VFS). The first plan B renamed `Verão` (NFD) to `Verão` (NFC) inside the
temp folder with `sync/move`, and on such a server this happened:

1. The server-side `DirMove` failed with "destination already exists": the server resolved the
   NFC path to the NFD directory.
2. rclone fell back to per-file moves, saw each file as "unchanged" (source and destination were
   the same physical file) and, since it was a *move*, **deleted the "source"**. That deleted the
   only copy.

In the test, 600 files vanished from the temp folder. The source was never touched, since
filebridge never writes to it.

Fix, in `transport-rclone`:
- `rclone rcd` runs with `--no-unicode-normalization`. `names` owns normalisation, and rclone
  compares bytes as-is.
- Every plan B rename goes through a unique intermediate name (`NFD → .fb-<n>-<job> → NFC`), so a
  rename never targets a name the server might alias to its own source. This doubles the
  renames.
- Directory renames run one at a time (`rclone serve sftp` crashes on concurrent directory
  renames, a test-server bug).

After the fix, plan A and plan B were both correct on a normalising server too: 2,000 files at
concurrency 1, 0 missing, 0 unexpected. **Plan A has no such risk:** it never renames or deletes
anything at the destination.

## Reproduce

```bash
cargo build --release
target/release/filebridge fixture /tmp/fx --files 2000 --size 16384 --dirs 20 --nfd-percent 30
rclone serve sftp /tmp/sftp-root --no-unicode-normalization --addr 127.0.0.1:2023 \
  --key host_key --authorized-keys authorized_keys &
python3 tools/bench/delay_proxy.py 2110 2023 10 &   # 20 ms RTT
target/release/filebridge bench --source /tmp/fx --dest-root bench \
  --sftp-host 127.0.0.1 --sftp-port 2110 --sftp-user test \
  --key-file client_key --known-hosts known_hosts --concurrency 8 --json out.json
```
