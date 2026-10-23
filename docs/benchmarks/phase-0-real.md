# Phase 0 benchmark — real server (template, not run yet)

The run that closes the Phase 0 gate (or reopens it): the same comparison as
[phase-0-simulated.md](phase-0-simulated.md), now from the author's machine to the real Nextcloud
server over SFTP, with subsets of the real `Backup` folder. [ADR 0007](../adr/0007-rename-in-transit-strategy.md)
stays *Proposed* until this page is filled in.

How to run it, step by step: [runbooks/real-server-tests.md § 1](../runbooks/real-server-tests.md#1-phase-0-real-benchmark).

| Mode | What it does |
|---|---|
| **baseline** | Plain `sync/copy` with the original names: what rclone/rsync does today. |
| **plan-a** | One `operations/copyfile` per file, straight to the NFC name (the transport called directly). |
| **engine** | Plan A sent through the Engine, as the app sends: analysis, SQLite manifest, batches, per-file state saving. |

Plan B is no longer in the code (ADR 0007), so it is not measured.

> **Fill in every `…`.** Use only dummy values for hosts, users and paths (`192.0.2.10`,
> `user`, `/tank/…`), never the real ones. The JSON files `filebridge bench --json` writes hold
> no host or path, only counts and timings, so they can be committed as they are.

## Setup

- Date: …
- filebridge commit: `…` (`git rev-parse --short HEAD`), built with `cargo build --release`.
- Client: … (CPU, threads `nproc`, RAM, OS `uname -srm`). Source disk: … (SSD / HDD / NVMe).
- Server: … (Proxmox node, or the Nextcloud LXC/VM reached directly). CPU …, RAM ….
  Destination disk: … (`lsblk -d -o NAME,ROTA,SIZE,MODEL`: `ROTA 1` = spinning disk),
  filesystem … (`df -T`), the same disk that holds Nextcloud's data: yes / no.
- Network: … (cable / Wi-Fi / VPN such as Tailscale). RTT: avg … ms, max … ms
  (`ping -c 20 192.0.2.10`). Throughput: … Mbit/s (`iperf3 -c 192.0.2.10 -t 10`, if available).
- rclone: v… (printed by `filebridge bench` and kept in the JSON). Server: OpenSSH … (`ssh -V`
  on the server).
- Destination: `--dest-root /tank/filebridge-bench` (dummy), outside Nextcloud's data folder so
  Nextcloud never indexes it.
- Concurrency: 8 (the default) unless a row says otherwise.
- Page cache: warmed before each run with `tar -cf - "$SRC" > /dev/null`: yes / no.

## Source subsets

A subset of `Backup` (one folder passed as `--source`), never the whole 500 GB. Two subsets
cover both ends: many small files (round trips dominate) and fewer large ones (bandwidth and disk
dominate).

| Subset | Files | Dirs | Size | < 1 MiB | 1–100 MiB | ≥ 100 MiB | NFC names to correct (files / dirs) | File paths that change | Non-UTF-8 skipped |
|---|---|---|---|---|---|---|---|---|---|
| A: small files (`…`) | … | … | … GB | … | … | … | … / … | … | … |
| B: large files (`…`) | … | … | … GB | … | … | … | … / … | … | … |

The NFC columns come from the `source:` line `filebridge bench` prints first (also `"source"` in
the JSON). Non-UTF-8 names should be 0: the `baseline` mode doesn't expect them, so any there
show up as `unexpected` in its row.

## Command

```bash
filebridge bench --source "$SRC" --dest-root /tank/filebridge-bench \
  --sftp-host 192.0.2.10 --sftp-port 22 --sftp-user root \
  --modes baseline,plan-a,engine --concurrency 8 \
  --json ~/fb-bench/A-c8.json   # B-c8.json for subset B
```

(`--key-file …` when not using the SSH agent.) Each mode writes to its own new folder
`/tank/filebridge-bench/filebridge-bench-<unix time>/<mode>`, which the bench **does not
remove**.

Raw results: `docs/benchmarks/phase-0-real/` (one JSON per run, copied from `~/fb-bench/`).

## Results

Every figure is one run unless noted. `failed`, `missing`, `unexpected` and `skipped` must be 0 in
every row for the run to count: `missing` are expected destination paths not found afterwards,
`unexpected` are paths found that weren't expected (e.g. a name left uncorrected), `skipped`
(engine only) are files left out because their name needed a decision, which its files/s then
doesn't count.

### Subset A — small files

| Mode | Files | MB | s | files/s | MB/s | failed | missing | unexpected | skipped | Stages (s) |
|---|---|---|---|---|---|---|---|---|---|---|
| baseline | … | … | … | … | … | … | … | … | 0 | copy … |
| plan-a | … | … | … | … | … | … | … | … | 0 | copy … |
| engine | … | … | … | … | … | … | … | … | … | analyse …, send … |

### Subset B — large files

| Mode | Files | MB | s | files/s | MB/s | failed | missing | unexpected | skipped | Stages (s) |
|---|---|---|---|---|---|---|---|---|---|---|
| baseline | … | … | … | … | … | … | … | … | 0 | copy … |
| plan-a | … | … | … | … | … | … | … | … | 0 | copy … |
| engine | … | … | … | … | … | … | … | … | … | analyse …, send … |

### Optional: higher concurrency, or the modes in reverse order

| Subset, concurrency, order | baseline | plan-a | engine | plan-a / baseline | engine / baseline |
|---|---|---|---|---|---|
| A, 32, normal | … files/s | … files/s | … files/s | … | … |
| A, 8, `engine,plan-a,baseline` | … files/s | … files/s | … files/s | … | … |

## Acceptance (ADR 0007)

Plan A is accepted when, on the real server, it stays **within about 0.8× of the baseline**, and
every run delivered every file under the expected name. The `engine` row is the one that
decides, since it is what the app ships; `plan-a` alone leaves the Engine's cost out.

Speed to compare: files/s for subset A, MB/s for subset B.

| Subset | plan-a / baseline | engine / baseline | ≥ 0.8? | 0 failed / missing / unexpected / skipped in every mode? |
|---|---|---|---|---|
| A (files/s) | … | … | yes / no | yes / no |
| B (MB/s) | … | … | yes / no | yes / no |

**Result:** passed / failed / passed with notes: …

- If both subsets pass: ADR 0007 becomes *Accepted* (plan A), and
  [09 — Roadmap](../09-roadmap.md) ticks "Measure speed on the real `Backup` folder" and "Record
  the result as an ADR".
- If `engine` fails but `plan-a` passes: the cost is in the Engine (manifest writes, batches),
  not in renaming; look there before reopening plan B.
- If `plan-a` fails: ADR 0007 is revised. Plan B comes back from git history
  (`crates/transport-rclone/src/rename.rs` and `temp_then_rename`), with the NFD/NFC hazard fix
  described in [phase-0-simulated.md](phase-0-simulated.md#hazard-found-plan-b-can-delete-files-on-a-normalising-destination).

## Reading

…

(What the numbers say: how close plan A and the engine are to the baseline, whether the analysis
stage matters at this size, how the RTT and the server's disk shaped the result, and anything
odd: retries, a slow first mode, a full disk.)
