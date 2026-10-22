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
| Differences between Nextcloud setups (Docker, AIO, Snap, LXC) | Medium | Start with the author's (LXC, via the Proxmox node or SSH into the LXC itself) and Nextcloud on the SSH host (VM, bare metal; ADR 0009); the detection wizard grows case by case |
| Nextcloud scan slow on HDD | Low | Scan only changed paths; show "waiting for disk" instead of looking stuck |

## Open decisions

- [ ] **Personal use only, or for distribution?** Changes the priority of code signing and
      documentation.
- [ ] **Do scheduled (recurring) sends and continuous sync enter the scope**, or only one-off
      sends? A daily window that holds back sends the user started is decided
      ([ADR 0010](adr/0010-bandwidth-limit-and-send-window.md)).
- [ ] **Final app name** (`filebridge` is provisional).
- [ ] **xxHash (fast) or SHA-256 (standard) for the manifest?** Note: the manifest can store the
      algorithm per entry, so this can default to one and still allow the other. rclone's SFTP
      backend computes remote hashes via `sha256sum`/`md5sum` over SSH; WebDAV/Nextcloud exposes
      only some checksums — check which ones the destination can compute before choosing.

When one is decided, tick it here and record an ADR in [adr/](adr/).
