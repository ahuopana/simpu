# Development

## Dev container

`.devcontainer/` provides Rust, Node, `cargo-xwin` (Windows cross-compile) and
Tauri's Linux dependencies. Open the repo in VS Code ("Reopen in Container") or
use any devcontainer-compatible tool.

## Everyday commands

    cargo test --workspace
    cargo clippy --workspace --all-targets -- -D warnings
    cargo fmt --all

## What can be tested where

| Area | Linux container / CI | Windows CI runner |
|---|---|---|
| `simpu-core` logic, jump chains, host keys, store | yes | yes |
| SSH/SFTP against an in-process test server | yes | yes |
| Frontend unit tests, Tauri `mockIPC` | yes | yes |
| GUI end-to-end (WebKitGTK under `xvfb`) | yes | - |
| GUI end-to-end (WebView2) | no | yes |
| Pageant, Credential Manager, Explorer drag-out | no | yes |
| Installer build and signing | cross-compile only | yes |

GitHub provides Windows as VM runners (`windows-latest`), not as containers.
See ADR 0004.

## Cross-compiling for Windows

    rustup target add x86_64-pc-windows-msvc
    cargo install cargo-xwin
    cargo xwin build --target x86_64-pc-windows-msvc -p simpu-core

The release build is produced on a Windows runner.
