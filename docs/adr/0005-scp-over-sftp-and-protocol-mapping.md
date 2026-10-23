# 0005. SCP and SFTP connections share one SSH/SFTP implementation

Status: accepted
Date: 2026-10-23

## Context
The connection tree offers ssh, telnet, sftp and scp protocols. OpenSSH 9+
implements the `scp` command on top of the SFTP protocol, and the legacy SCP
protocol has known weaknesses (filename handling by the remote side).

## Decision
- `ssh`, `sftp` and `scp` connections all open an SSH session; file transfer
  (drag-and-drop, browsing) always uses the SFTP subsystem.
- No separate legacy SCP wire implementation. Servers without an SFTP
  subsystem are not supported for file transfer.
- `telnet` is a separate plaintext transport (`simpu_core::telnet`); the UI
  shows a warning for it.

## Consequences
One code path to test and secure. Hosts that only offer legacy SCP will not
work for transfers; this can be revisited with a new ADR if it matters.
