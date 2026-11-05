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

## Remote file browser

"📁 Files" opens a panel next to the terminals that follows the focused pane's SSH session (SFTP):
breadcrumbs, up/refresh, folders first. Double-click a folder to enter it, a file to download it (a
folder dialog asks where to). Hover a row for download / rename / delete (folders must be empty);
"＋ Folder" creates one; "⬆ Upload…" or dropping files on the panel uploads into the current folder.
Dropping files on a terminal still uploads to the home directory. Folders work too: the ⬇ on a folder row downloads it recursively (into a folder of the same name inside
the one you pick), and "⬆ Folder…" or dropping a folder on the panel uploads it recursively; progress is one
aggregated bar. Symbolic links are skipped, and file names from the server that could write outside the target
(`..`, separators, `:` and other characters Windows reserves) abort the download before anything is written.
Dragging files *out* of the panel to Explorer is not supported (it needs the file on disk before the drag
starts; the download button covers this).

## Keys, passphrases and ssh-agent

- Local key file: provider `file`, item = path (`~/.ssh/id_ed25519`), kind key file. If the key is
  encrypted Simpu asks for the passphrase when connecting.
- ssh-agent / Pageant: provider `ssh-agent / Pageant` (`agent`). Item is empty, or a fingerprint
  (`SHA256:...`, from `ssh-add -l`) to pick one key. On Windows start the "OpenSSH Authentication Agent"
  service (or Pageant) and add keys with `ssh-add`.

## Settings

"⚙ Settings" edits `settings.toml` in the config directory (Windows: `%APPDATA%\dev.simpu.app\`):
KeePassXC database / key file / `keepassxc-cli` path, the `bw` path, keepalive interval and misses,
connect timeout, and the automatic-reconnect delays (first delay, longest delay, attempt limit). Values are
validated in the dialog and again in the core; invalid settings are never written. Network and reconnect
changes apply to sessions opened afterwards; changing a password-manager setting restarts its plugin (open
sessions keep the credentials they already resolved). The `KEEPASSXC_DB`, `KEEPASSXC_KEYFILE` and
`KEEPASSXC_CLI` environment variables still work as a fallback when the setting is empty.

## Using the KeePassXC plugin

Install KeePassXC (it ships `keepassxc-cli`). Set the database path (and optionally the
key file and CLI path) under ⚙ Settings (or, for scripted setups, `KEEPASSXC_DB`, `KEEPASSXC_KEYFILE`,
`KEEPASSXC_CLI`). In a connection choose provider `keepassxc`; item =
`Group/Entry` for a password, `Group/Entry#attachment-name` for a private key
stored as an attachment (kind `keyfile`; without `#` the key is read from Notes).
The master password is asked in-app on first use.

## Port forwarding and reconnect

In the connection editor, **Forwards** takes one line per forward in `ssh` style:
`L 5433:db.internal:5432` (local), `R 9000:localhost:3000` (remote), `D 1080` (SOCKS5);
prefix the port with a bind address (`L 0.0.0.0:8080:web:80`) to expose it to other
machines. **On disconnect** chooses auto / ask / close (default: auto when forwards
exist, otherwise ask). **Tunnel only** skips the shell (like `ssh -N`). The
"⇄ Forwards" toolbar button shows live forwards of the focused session and lets you
add or stop them. `LocalForward`, `RemoteForward`, `DynamicForward` and
`SessionType none` are imported from `~/.ssh/config`.

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

## Building the Linux packages

    cargo build --release -p simpu-plugin-bitwarden -p simpu-plugin-keepassxc
    cd crates/simpu-app
    ../../ui/node_modules/.bin/tauri build --bundles deb,appimage --config tauri.bundle.linux.conf.json

Output: `target/release/bundle/deb/*.deb` and `target/release/bundle/appimage/*.AppImage`. The plugins are placed
next to the app executable (`/usr/bin` in the .deb, `usr/bin` inside the AppImage) because the app looks for them
there. The AppImage build downloads `linuxdeploy` from GitHub on first use and needs `libfuse2` (or run the tools
with `APPIMAGE_EXTRACT_AND_RUN=1`). A full release build takes about 4 minutes; delete `target/debug` first if
the disk is tight. To eyeball the UI headlessly: `xvfb-run` the binary and capture with `import -window root`.

## Cross-compiling for Windows

    rustup target add x86_64-pc-windows-msvc
    cargo install cargo-xwin
    cargo xwin build --target x86_64-pc-windows-msvc -p simpu-core

The release build is produced on a Windows runner.
