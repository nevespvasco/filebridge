# ADR 0001 — Tauri 2 + Vue 3 + Rust core

- Status: Accepted
- Date: 2026-10-21

## Context
The app must run on Linux, Windows and macOS, walk hundreds of thousands of files quickly, run a
long-lived transfer engine, and ship native sidecars (rclone). The author's main stack is Vue/TS.

## Decision
Use **Tauri 2** as the desktop shell, **Vue 3 + TypeScript + Vite** (Pinia) for the UI, and a
**Rust (tokio)** core. UI ↔ Rust types are generated with `specta`/`tauri-specta`.

## Consequences
- Small installers on all three OSes; native sidecar support.
- Strongly typed plugin contracts in Rust.
- Rejected: **Electron** (heavy, Node core), **React** (works, no gain over Vue for this author),
  **Go** core (good, but doesn't integrate with Tauri).
- Rust adds a learning cost on the core; mitigated by keeping plugins small and contracts explicit.
