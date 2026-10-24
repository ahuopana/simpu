# Development

## Dev container

`.devcontainer/` provides Rust, Node, `cargo-xwin` (Windows cross-compile) and
Tauri's Linux dependencies. Open the repo in VS Code ("Reopen in Container") or
use any devcontainer-compatible tool.

## Everyday commands

    cargo test --workspace
    cargo clippy --workspace --all-targets -- -D warnings
    cargo fmt --all

## Layout

| Path | Purpose |
|---|---|
| `crates/simpu-core` | Protocols (SSH, SFTP, telnet), jump chains, host keys, credentials, session manager |
| `crates/simpu-plugin-api` | Plugin wire protocol and server loop |
| `plugins/bitwarden` | Bitwarden credential plugin (uses the `bw` CLI) |
| `crates/simpu-app` | Tauri shell (thin adapter over `Manager`) |
| `ui/` | Vite + TypeScript + xterm.js frontend |

## Running things

    # Rust
    cargo test --workspace

    # Frontend (needs `cd ui && npm ci` once)
    npm test            # unit + contract tests
    npm run e2e         # Playwright against the mock backend
                        # (set PW_CHROMIUM=/path/to/chrome if the bundled version differs)
    npm run dev         # UI in a browser with the mock backend

    # Desktop app (needs the UI built: `npm run build` in ui/)
    cargo build -p simpu-app && ./target/debug/simpu-app

Tauri's Linux dependencies: `libwebkit2gtk-4.1-dev libgtk-3-dev
libayatana-appindicator3-dev librsvg2-dev libxdo-dev` (preinstalled in the dev
container).

## Using the Bitwarden plugin

Install the Bitwarden CLI (`bw`), run `bw login` once, then
`export BW_SESSION=$(bw unlock --raw)` before starting Simpu. The plugin
executable `simpu-plugin-bitwarden` must sit next to the app executable. In a
connection, set the secret to provider `bitwarden`, item = the Bitwarden item
id, kind = `password` (login password) or `keyfile` (private key stored in the
item's notes). Jump hosts use the same mechanism per hop.

## Using the KeePassXC plugin

Install KeePassXC (it ships `keepassxc-cli`). Before starting Simpu set
`KEEPASSXC_DB` to the `.kdbx` path (optionally `KEEPASSXC_KEYFILE`,
`KEEPASSXC_CLI`). In a connection choose provider `keepassxc`; item =
`Group/Entry` for a password, `Group/Entry#attachment-name` for a private key
stored as an attachment (kind `keyfile`; without `#` the key is read from Notes).
The master password is asked in-app on first use.

## Log capture

Use "● Record" on a pane, then "Export log…": everything or the last N commands
(detected from shell prompts; override the pattern under "advanced"), optional
anonymization (usernames, hostnames/IPs, passwords/keys, custom regexes) with a
preview, then copy or save. Passwords Simpu itself knows (vault, prompts) are
masked before the capture extension sees them (ADR 0007).

## Debugging CI

Job logs may not be downloadable from restricted environments. The smoke test
prints failures as annotations: `gh api repos/OWNER/REPO/check-runs/ID/annotations`.

## What can be tested where

| Area | Linux container / CI | Windows CI runner |
|---|---|---|
| `simpu-core` logic, jump chains, host keys, store | yes | yes |
| SSH/SFTP against an in-process test server | yes | yes |
| Frontend unit tests, Tauri `mockIPC` | yes | yes |
| UI end-to-end (Playwright, Chromium, mock backend) | yes | - |
| App starts (WebKitGTK under `xvfb` / WebView2) | yes | yes |
| Pageant, Credential Manager, Explorer drag-out | no | yes |
| Installer build and signing | cross-compile only | yes |

GitHub provides Windows as VM runners (`windows-latest`), not as containers.
See ADR 0004.

## Cross-compiling for Windows

    rustup target add x86_64-pc-windows-msvc
    cargo install cargo-xwin
    cargo xwin build --target x86_64-pc-windows-msvc -p simpu-core

The release build is produced on a Windows runner.
