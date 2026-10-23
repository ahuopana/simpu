# 0004. Test strategy: fakes, in-process servers, two CI jobs

Status: accepted
Date: 2026-10-23

## Context
Manual testing on Windows is slow. GitHub offers Windows VM runners but not
Windows containers on a Linux host.

## Decision
- OS-specific functionality sits behind traits in `simpu-core::platform`, with
  Linux fakes.
- SSH behaviour is tested against an in-process `russh` server (no Docker).
- CI has a Linux job (tests, clippy, fmt) and a Windows job (build, tests, later
  GUI smoke test and installer).

## Consequences
Most regressions are caught on Linux in seconds. Windows-only behaviour is
covered by the Windows job rather than by hand.
