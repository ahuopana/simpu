# 0001. Rust core with Tauri 2 and xterm.js

Status: accepted
Date: 2026-10-23

## Context
We need a Windows desktop SSH client with tabs, split panes and drag-and-drop.
.NET is excluded by the owner. Building blocks should not be Windows-only.
A terminal emulator is the largest hidden cost.

## Decision
Rust core (`russh` for SSH/SFTP), Tauri 2 shell, xterm.js terminal.

## Consequences
Small installer, a mature terminal emulator, and a core testable on Linux.
Tauri uses WebView2 on Windows and WebKitGTK on Linux, so rendering can
differ; Windows CI covers WebView2.
