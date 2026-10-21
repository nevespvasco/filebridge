# apps/desktop

Tauri 2 + Vue 3 + TypeScript + Vite desktop app. **Lands in:** Phase 1.

## Responsibility
- `src/` — the interface: the 5-step "New transfer" wizard, Jobs, Job detail, Destinations,
  Integrations, Repair destination, Settings. Pinia stores, vue-i18n (pt, en).
- `src-tauri/` — Tauri commands and events. **A thin client of `filebridge-service`** over
  `crates/ipc` ([ADR 0008](../../docs/adr/0008-service-ipc.md)): no business logic here. Starts the
  service if its socket is missing, and manages the tray icon, notifications, autostart and the
  sidecar declarations (rclone, filebridge-service).

## Rules
- All Rust ↔ TS types come from `tauri-specta` into `src/bindings/` — never hand-written.
- Closing the window never stops a send.
- Destructive actions always confirm and show what will happen.

## Allowed dependencies (src-tauri)
`ipc`, `tauri`, `tauri-specta`. Never `core` or a plugin crate directly.

See [docs/06-interface.md](../../docs/06-interface.md).
