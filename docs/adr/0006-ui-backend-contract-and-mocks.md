# 0006. UI talks to a Backend interface; mock backend for browser tests

Status: accepted
Date: 2026-10-23

## Context
The real GUI (Tauri) cannot be driven by Playwright, and GitHub provides
Windows only as VMs. We want fast, repeatable UI tests without manual steps.

## Decision
- The UI depends on the `Backend` interface (`ui/src/api.ts`).
  `tauriBackend.ts` calls Rust commands; `mockBackend.ts` is an in-memory
  implementation used by `vite dev` and by Playwright (Chromium) tests.
- A contract test (`ui/tests/contract.test.ts`) checks that every `invoke(...)`
  in `tauriBackend.ts` matches a `#[tauri::command]` (name and arguments).
- Application logic lives in `simpu_core::manager::Manager` behind an `Events`
  trait, tested with an in-process SSH/SFTP server. `crates/simpu-app` stays
  a thin adapter.
- CI smoke tests start the real app (Xvfb/WebKitGTK on Linux, WebView2 on
  Windows) and fail if it exits early.

## Consequences
UI behaviour, Rust logic and the boundary between them are each tested
automatically. What is not covered: pixel-level rendering differences between
WebKitGTK/WebView2 and Chromium, and real-OS drag-and-drop. Revisit with
`tauri-driver` (WebDriver) if regressions slip through.
