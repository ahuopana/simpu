# 0007. Session-output hooks for built-in extensions; redaction happens in the host

Status: accepted
Date: 2026-10-24

## Context
We want extensions such as log capture. Terminal output can contain secrets the
host itself supplied (vault passwords, typed passwords, vault master password).
Extensions should not be able to read those.

## Decision
- `simpu_core::ext` defines `SessionObserver` (output, closed) and
  `ExtensionHost`. First-party extensions run in-process; external plugins keep
  the out-of-process JSON-RPC model (ADR 0002) and may get a similar hook later.
- Before any observer sees bytes, `simpu_core::redact::StreamRedactor` replaces
  every secret registered in the manager's `SecretSet` with `⟦secret⟧`,
  including secrets split across chunks. The terminal (xterm) still receives the
  raw bytes.
- Secrets are registered when resolved: vault/file passwords and keys, prompted
  passwords, and the vault unlock master password. Secrets shorter than 3 bytes
  are ignored.
- Observers do not receive keystrokes (no input hook), to keep the privacy
  surface small.

## Consequences
Extensions cannot recover host-known secrets. Secrets the host never saw
(typed into `sudo` or a remote application) are not masked; export-time
anonymization with user regex rules covers those on a best-effort basis.
