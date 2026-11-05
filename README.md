<p align="center"><img src="branding/simpu-icon.svg" width="128" alt="Simpu logo"></p>

# Simpu

A Windows SSH client with a connection tree, tabbed and split sessions,
drag-and-drop file transfer, jump hosts, and a plugin interface
(first plugin: Bitwarden). Named after *simpukka*, Finnish for shell.

**Status:** early development (pre-1.0). The core protocols, UI and Bitwarden
plugin work in tests; there is no release yet.

## Features (planned for 1.0)

- SSH, SFTP, SCP and Telnet
- Connection tree: name, host, user, protocol, folders, search
- Multiple sessions in tabs; split panes
- Drag-and-drop file transfer and a remote file browser (SFTP panel: browse, upload, download, rename, delete; whole folders too)
- Authentication: passwords, key files (with passphrase prompts), ssh-agent / Pageant
- Jump host chains, with credentials from a password manager
- Port forwarding (local, remote, SOCKS5), tunnel-only connections
- Keepalive and per-connection reconnect policy (automatic, ask, or close)
- Plugins: Bitwarden and KeePassXC supply passwords and key files; a settings screen configures them
- Log capture: record a session, export the last N commands or everything,
  with optional anonymization of usernames, hosts and secrets

## Install

No release yet; installers are built by CI (Actions artifacts of the `main` branch).

- **Windows 10/11 x64:** run the NSIS installer (`simpu-windows-installer`).
- **Linux x64 (Debian/Ubuntu):** `sudo apt install ./Simpu_0.1.0_amd64.deb` (pulls in `libwebkit2gtk-4.1-0` and
  `libgtk-3-0`), then start **Simpu** from the application menu or run `simpu-app`.
- **Linux x64 (any distro):** `chmod +x Simpu_0.1.0_amd64.AppImage && ./Simpu_0.1.0_amd64.AppImage`
  (self-contained, about 80 MB). If your system has no FUSE 2, use `--appimage-extract-and-run`.

Both Linux packages include the Bitwarden and KeePassXC plugins. Settings and connections are stored in
`~/.config/dev.simpu.app/` (Windows: `%APPDATA%\dev.simpu.app\`).

## Build from source

See [docs/development.md](docs/development.md).

## Documentation

- [Development environment and testing](docs/development.md)
- [Architecture decisions (ADRs)](docs/adr/)
- [Working with Claude in this repo](CLAUDE.md)

## License

Apache-2.0, see [LICENSE](LICENSE).
