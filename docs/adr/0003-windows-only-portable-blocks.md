# 0003. Windows-only target, portable building blocks

Status: accepted
Date: 2026-10-23

## Context
The owner only needs Windows, but wants libraries that are not tied to Windows.

## Decision
Ship and support Windows only. Use cross-platform libraries and keep Windows
specifics in small adapters.

## Consequences
Linux CI can test almost everything. No Linux/macOS builds are promised.
