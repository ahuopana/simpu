import { describe, expect, it } from "vitest";
import { defaultSettings, validateSettings } from "../src/settings";

describe("settings validation", () => {
  it("defaults are valid", () => expect(validateSettings(defaultSettings())).toEqual([]));

  it("reports each problem; 0 turns keepalives off", () => {
    const s = defaultSettings();
    s.network.keepalive_secs = 0;
    expect(validateSettings(s)).toEqual([]);
    s.network.keepalive_secs = 2;
    s.network.keepalive_max = 0;
    s.network.connect_timeout_secs = 1;
    s.reconnect.base_secs = 0;
    s.reconnect.max_secs = 5000;
    s.reconnect.max_attempts = 5000;
    expect(validateSettings(s)).toHaveLength(6);
  });

  it("max delay may not be below the first delay; non-integers are rejected", () => {
    const s = defaultSettings();
    s.reconnect.base_secs = 10;
    s.reconnect.max_secs = 5;
    expect(validateSettings(s)).toHaveLength(1);
    const t = defaultSettings();
    t.network.keepalive_secs = 7.5;
    expect(validateSettings(t)).toHaveLength(1);
  });
});
