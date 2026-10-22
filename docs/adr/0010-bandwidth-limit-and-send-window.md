# ADR 0010 — One app-wide bandwidth limit and a daily send window

- Status: Accepted
- Date: 2026-10-22

## Context
Settings showed a bandwidth limit and a send schedule as placeholders. The per-job limit that did
exist (`SendOptions.bwlimit`, `filebridge send --bwlimit`) was broken by design: `transport-rclone`
passed it to rclone's `core/bwlimit`, which is **global to the rcd process**, and never cleared it.
One job's limit therefore carried over to every later job until the rcd restarted, and it slowed
any job running alongside it.

"Scheduled sends" is a Phase 3 item and an [open decision](../10-risks-and-open-decisions.md#open-decisions).
That item is about sends that start on their own (recurring sends, continuous sync). What the
Settings screen promises is narrower: hours when sends that the user started may run, e.g. only at
night.

## Decision
- **Settings live in the service**: `AppSettings { bandwidth_limit, send_window }` (`crates/model`),
  saved as JSON under the key `app` in the existing `settings` table, read and written through the
  ipc operations `get_settings` / `save_settings`. `filebridge settings` sets them from the CLI.
- **One bandwidth limit for the whole app**, in bytes per second. The per-job `bwlimit` is removed
  from `SendOptions` and `TransferOptions`. The `Transport` contract gets
  `set_bandwidth_limit(Option<u64>)`, with a default no-op. `core` calls it on every registered
  transport when the settings are saved and when the host opens the engine. rclone receives
  `core/bwlimit` with `"<n>B"`, or `"off"` to clear it, so running transfers slow down or speed up
  at once.
- **A daily send window** in the service machine's local time: `start_minute` included,
  `end_minute` excluded, and `start > end` crosses midnight. It **holds back only the file
  transfer**: verification (remote hashes) and the integration (e.g. `occ files:scan`) use little
  bandwidth and may finish outside the window. Files resent after a failed verification go through
  the transfer, so they still wait.
- A new job state, **`Waiting`**, is a running state (pause, cancel and startup recovery apply). It
  can be entered from Transferring and Retrying, and can go to Transferring or Paused. The runner
  checks the window before each transfer stage and races each transport call against the window
  closing. Closing works like a pause: finished files are kept, files in flight go again. While
  Waiting it emits `SendWindowWait { opens_at }` and re-checks at least every minute. That covers
  system suspend (the monotonic clock stops), DST and clock changes. A settings change is picked up
  at once through a `watch` channel.
- While Waiting the job **lets go of the sleep inhibitor**, so a day-long wait doesn't keep the
  computer awake.

## Consequences
- The computer must be on and awake when the window opens; filebridge doesn't wake it. A laptop
  that sleeps at night won't send at night.
- A per-job limit is gone. Bringing it back would need rclone per-transfer limits, which the rc API
  doesn't offer, or one rcd per job.
- Stored job options from before this change still deserialize; their `bwlimit` is ignored.
- `PROTOCOL_VERSION` 4.
- Recurring or automatic sends stay an open decision for Phase 3.
