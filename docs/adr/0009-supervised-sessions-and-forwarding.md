# 0009. Supervised sessions: forwarding, keepalive and reconnect policies

Status: accepted
Date: 2026-10-24

## Context
Users need SSH tunnels (local, remote, SOCKS) and sessions that survive
network drops. Plain shells and tunnels want different behaviour: a tunnel should
come back by itself, while for an interactive shell the user usually wants to decide.

## Decision
- A connection stores `forwards` (`local`/`remote`/`dynamic`), `reconnect`
  (`auto|ask|off`) and `no_shell` (tunnel only). Default policy: `auto` when the
  connection has forwards, otherwise `ask`. All fields are optional, so existing
  `connections.toml` files load unchanged.
- Forwards can also be added and stopped on a live session; they are remembered
  for the session's lifetime and restored on reconnect with stable ids.
- One supervisor task per SSH session owns the shell pump, the forwards and the
  connection state. Keepalives (30 s, 3 misses by default) and the transport's
  disconnect signal detect dead links; a connect timeout (not counting time spent
  in host-key prompts) prevents hung attempts.
- `auto`: retry with exponential backoff (1 s doubling to 60 s, optional attempt
  limit). Host-key changes/rejections, authentication failures and bad keys are not
  retried; the session falls back to waiting for the user. `ask`: show
  Reconnect/Close. `off`: close. A shell that exits normally never reconnects.
- Reconnecting reuses the already resolved credentials, so vault and password
  prompts do not repeat, and opens a fresh shell in the same pane (remote state is
  lost; a status line marks the reconnect).
- Forwards bind to `127.0.0.1` by default; other addresses are allowed but flagged
  in the UI. SOCKS5 supports CONNECT without authentication only.

## Consequences
Telnet sessions are not supervised yet (no reconnect). Remote forwards require
`AllowTcpForwarding` on the server; refusals are shown per forward. The supervisor
logic is tested through `Manager` with a controllable in-process server (kill
connections, black-holed link), on Linux and Windows CI.
