# 0010. ssh-agent authentication and key passphrases

Status: accepted
Date: 2026-11-02

## Context
Most real keys are passphrase-protected or live in an agent. Windows users have
two agents: the OpenSSH agent service (named pipe) and Pageant.

## Decision
- Secret provider `agent` selects agent authentication. The optional item is a key
  fingerprint (`SHA256:...`) restricting which agent key is offered; empty means
  "try every key". On Windows the default endpoint is the OpenSSH agent pipe
  (`\\.\pipe\openssh-ssh-agent`), then Pageant; elsewhere `SSH_AUTH_SOCK`.
- Private keys (from the `file` provider, a vault, ...) are checked before
  connecting. If encrypted, the user is asked for the passphrase (up to 3 tries);
  the passphrase is kept in memory for the session (reconnects reuse it) and masked
  in captured output like other known secrets (ADR 0007).
- Agent certificates are not offered yet (plain public keys only).

## Consequences
No key material leaves the agent. Authentication errors from the agent are reported
as `ssh-agent: ...` (unreachable, no keys, fingerprint mismatch) rather than a
generic failure. The Windows endpoints are exercised only by the Windows CI build
(no agent runs there); the Unix socket path is covered by an in-process agent in tests.
