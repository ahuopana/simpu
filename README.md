# Simpu

A Windows SSH client with a connection tree, tabbed and split sessions,
drag-and-drop file transfer, jump hosts, and a plugin interface
(first plugin: Bitwarden). Named after *simpukka*, Finnish for shell.

**Status:** early development (pre-1.0). Not usable yet.

## Features (planned for 1.0)

- SSH, SFTP, SCP and Telnet
- Connection tree: name, host, user, protocol, folders, search
- Multiple sessions in tabs; split panes
- Drag-and-drop file transfer
- Jump host chains, with credentials from a password manager
- Plugins; the first one reads passwords and key files from Bitwarden

## Install

No release yet. Windows 10/11 x64 will be supported; the installer will be
published on the GitHub Releases page.

## Build from source

See [docs/development.md](docs/development.md).

## Documentation

- [Development environment and testing](docs/development.md)
- [Architecture decisions (ADRs)](docs/adr/)
- [Working with Claude in this repo](CLAUDE.md)

## License

Apache-2.0, see [LICENSE](LICENSE).
