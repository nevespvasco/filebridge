# apps/desktop

Tauri 2 + Vue 3 + TypeScript + Vite desktop app. **Lands in:** Phase 1.

## Responsibility
- `src/` — the interface: the 5-step "New transfer" wizard, Jobs, Job detail, Destinations,
  Integrations, Repair destination, Settings. Pinia stores, vue-i18n (pt, en).
- `src-tauri/` — Tauri commands and events. **Only a bridge to `core`**: no business logic here.
  Builds the `Registry` (registers `transport-rclone`, `integration-nextcloud`), manages the tray
  icon, notifications, autostart and the rclone sidecar declaration.

## Rules
- All Rust ↔ TS types come from `tauri-specta` into `src/bindings/` — never hand-written.
- Closing the window never stops a send.
- Destructive actions always confirm and show what will happen.

## Allowed dependencies (src-tauri)
`core`, `transport-rclone`, `integration-nextcloud` (to register them), `tauri`, `tauri-specta`.

See [docs/06-interface.md](../../docs/06-interface.md).
