# 0008. KeePassXC provider uses keepassxc-cli; browser-protocol proxy later

Status: accepted
Date: 2026-10-24

## Context
KeePassXC offers a CLI and a browser-integration protocol (native-messaging
proxy with an association handshake). We want a second credential provider
that works headless and can be tested automatically.

## Decision
- `plugins/keepassxc` is an out-of-process plugin (ADR 0002) that shells out to
  `keepassxc-cli` (`show -s -a Password|Notes`, `attachment-export --stdout`).
- The master password arrives via `vault.unlock`, is validated by opening the
  database (`ls`), is kept in plugin memory only, and is passed to the CLI on
  stdin (never argv or environment). Optional key file via `KEEPASSXC_KEYFILE`.
- Database path via `KEEPASSXC_DB` (settings UI is a follow-up).
- Item syntax: `Group/Entry` (password; key file from Notes) or
  `Group/Entry#attachment` (key file from an attachment).
- Tests use a portable fake CLI (`fake-keepassxc-cli`), like `fake-bw`.
- A browser-protocol (proxy) provider may be added later as a separate ADR.

## Consequences
Each secret fetch spawns the CLI (database decryption cost per call). Requires
`keepassxc-cli` to be installed. No dependency on a running KeePassXC GUI.
