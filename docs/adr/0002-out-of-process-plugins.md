# 0002. Out-of-process plugins over JSON-RPC

Status: accepted
Date: 2026-10-23

## Context
Plugins (starting with Bitwarden) handle passwords and keys. They must be
isolatable, language-agnostic and versioned.

## Decision
Plugins are separate executables speaking line-delimited JSON-RPC 2.0 over
stdio. The first extension point is a credential provider (`get(item)`).
Plugins declare permissions in a manifest.

## Consequences
A crashing or malicious plugin cannot corrupt the host's memory; secrets cross
a process boundary and must be zeroized by the host after use. Slightly higher
latency per call.
