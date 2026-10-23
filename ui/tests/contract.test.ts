// Guards the UI <-> Rust boundary: every `invoke("cmd", {args})` in tauriBackend.ts
// must name a #[tauri::command] in crates/simpu-app with the same argument names
// (Rust snake_case params arrive as camelCase).
import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

const rust = readFileSync(new URL("../../crates/simpu-app/src/lib.rs", import.meta.url), "utf8");
const ts = readFileSync(new URL("../src/tauriBackend.ts", import.meta.url), "utf8");

const camel = (s: string) => s.replace(/_(\w)/g, (_, c: string) => c.toUpperCase());

function rustCommands(): Map<string, string[]> {
  const out = new Map<string, string[]>();
  const re = /#\[tauri::command\]\s*(?:async\s+)?fn\s+(\w+)\s*\(([^)]*)\)/g;
  for (const m of rust.matchAll(re)) {
    const params = m[2]!
      .replace(/\w+:\s*State<[^>]*>/g, "")
      .split(",")
      .map((p) => p.trim())
      .filter(Boolean)
      .map((p) => camel(p.split(":")[0]!.trim()));
    out.set(m[1]!, params.sort());
  }
  return out;
}

function tsInvokes(): Map<string, string[]> {
  const out = new Map<string, string[]>();
  for (const m of ts.matchAll(/invoke\("(\w+)"(?:,\s*\{([^}]*)\})?\)/g)) {
    const args = (m[2] ?? "").split(",").map((a) => a.trim().split(":")[0]!.trim()).filter(Boolean);
    out.set(m[1]!, args.sort());
  }
  return out;
}

describe("tauri command contract", () => {
  const r = rustCommands();
  const t = tsInvokes();

  it("finds commands on both sides", () => {
    expect(r.size).toBeGreaterThan(5);
    expect(t.size).toBeGreaterThan(5);
  });

  it("every UI invoke has a Rust command with identical arguments", () => {
    for (const [name, args] of t) {
      expect(r.has(name), `Rust command ${name} missing`).toBe(true);
      expect(args, `arguments of ${name}`).toEqual(r.get(name));
    }
  });

  it("every registered Rust command is registered in the handler", () => {
    const handler = /generate_handler!\[([^\]]*)\]/.exec(rust)![1]!;
    for (const name of r.keys()) expect(handler, name).toContain(name);
  });
});
