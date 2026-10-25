# 10 — Risks & open decisions

The biggest risk is per-file renaming with rclone: copying file by file with a different name is
slow for hundreds of thousands of files. Validating this is the first prototype, before any
interface.

## Risks

| Risk | Impact | Mitigation |
|---|---|---|
| Renaming during send is slow in rclone | High | Prototype in week 1 (Phase 0). Plan B: send with the original name to a temporary folder, rename in batch at the destination, then move |
| Owner and permissions (`chown`) not supported over SFTP without root | Medium | Direct mode with a dedicated user in the right group, or remote `chown` after the send |
| Code signing (macOS notarization, Windows SmartScreen) | Medium | Developer accounts; until then, builds for personal use only |
| rclone licence (MIT) when distributing the binary | Low | Include the licence; no issue for MIT |
| Differences between Nextcloud setups (Docker, AIO, Snap, LXC) | Medium | Detected: Proxmox LXC (through the node or SSH into the LXC), Nextcloud on the SSH host (VM, bare metal; ADR 0009), Docker (official image), AIO and the snap ([05](05-integration-nextcloud.md#setups-supported)); Docker and snap detection is checked against fixtures only, not a real server yet |
| Nextcloud scan slow on HDD | Low | Scan only changed paths; show "waiting for disk" instead of looking stuck |
| Phones (Phase 4): librclone is experimental, Go cross-compiling, iOS background limits | Medium | See [11 — Mobile](11-mobile.md#risks); plan B is a native SFTP/WebDAV transport crate |

## Open decisions

- [x] **Personal use only, or for distribution?** Personal use, for now
      ([ADR 0018](adr/0018-scope-personal-use-scheduled-and-copy-only-sync.md)).
- [x] **Do scheduled (recurring) sends and continuous sync enter the scope?** Both: scheduled
      sends in Phase 3, continuous sync in Phase 5, copy-only (never deletes at the destination;
      a rename at the source is a new file). [ADR 0018](adr/0018-scope-personal-use-scheduled-and-copy-only-sync.md).
      The daily window is [ADR 0010](adr/0010-bandwidth-limit-and-send-window.md).
- [x] **Final app name**: `filebridge` ([ADR 0018](adr/0018-scope-personal-use-scheduled-and-copy-only-sync.md),
      [ADR 0032](adr/0032-keep-the-name-filebridge.md)).
- [x] **xxHash or SHA-256 for the manifest?** SHA-256
      ([ADR 0017](adr/0017-sha256-for-the-manifest.md)).

When one is decided, tick it here and record an ADR in [adr/](adr/).
