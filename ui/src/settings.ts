import type { Settings } from "./api";

export function defaultSettings(): Settings {
  return {
    keepassxc: { database: "", keyfile: "", cli: "" },
    bitwarden: { cli: "" },
    network: { keepalive_secs: 30, keepalive_max: 3, connect_timeout_secs: 20 },
    reconnect: { base_secs: 1, max_secs: 60, max_attempts: 0 },
  };
}

const int = (n: number) => Number.isInteger(n);

/** Mirrors `Settings::validate` in simpu-core so the dialog can answer immediately. */
export function validateSettings(s: Settings): string[] {
  const e: string[] = [];
  const n = s.network;
  if (!int(n.keepalive_secs) || (n.keepalive_secs !== 0 && (n.keepalive_secs < 5 || n.keepalive_secs > 3600)))
    e.push("keepalive interval must be 0 (off) or 5-3600 seconds");
  if (!int(n.keepalive_max) || n.keepalive_max < 1 || n.keepalive_max > 20) e.push("keepalive misses must be 1-20");
  if (!int(n.connect_timeout_secs) || n.connect_timeout_secs < 3 || n.connect_timeout_secs > 300) e.push("connect timeout must be 3-300 seconds");
  const r = s.reconnect;
  if (!int(r.base_secs) || r.base_secs < 1 || r.base_secs > 600) e.push("first retry delay must be 1-600 seconds");
  if (!int(r.max_secs) || r.max_secs < r.base_secs || r.max_secs > 3600) e.push("longest retry delay must be between the first delay and 3600 seconds");
  if (!int(r.max_attempts) || r.max_attempts < 0 || r.max_attempts > 1000) e.push("retry attempts must be 0 (unlimited) to 1000");
  return e;
}
