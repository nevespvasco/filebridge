# sidecars

Per-platform **rclone** binaries shipped as Tauri sidecars. **Lands in:** Phase 0/1.

- Binaries go in `sidecars/bin/` and are **not committed** (see `.gitignore`); a script
  (Phase 1) downloads a pinned rclone version and verifies its checksum.
- Tauri naming: `rclone-<target-triple>[.exe]`, e.g. `rclone-x86_64-unknown-linux-gnu`,
  `rclone-aarch64-apple-darwin`, `rclone-x86_64-pc-windows-msvc.exe`.
- rclone is **MIT-licensed** — include its `LICENSE` in distributed builds.

See [ADR 0002](../docs/adr/0002-rclone-behind-transport.md).
