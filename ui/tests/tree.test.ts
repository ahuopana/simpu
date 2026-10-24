import { describe, expect, it } from "vitest";
import type { NodeDto } from "../src/api";
import { filterTree, findConnection } from "../src/tree";

const tree: NodeDto[] = [
  { type: "folder", name: "prod", children: [{ type: "connection", id: "1", name: "db", protocol: "ssh", host: "db.example.com", username: "root" }] },
  { type: "connection", id: "2", name: "switch", protocol: "telnet", host: "10.0.0.1", username: "admin" },
];

describe("tree", () => {
  it("filters by name/host/user and prunes empty folders", () => {
    expect(filterTree(tree, "")).toBe(tree);
    expect(filterTree(tree, "EXAMPLE")).toHaveLength(1);
    expect(filterTree(tree, "admin")).toHaveLength(1);
    expect(filterTree(tree, "zzz")).toHaveLength(0);
  });
  it("finds nested connections", () => {
    expect(findConnection(tree, "1")?.name).toBe("db");
    expect(findConnection(tree, "9")).toBeUndefined();
  });
});

import { newId, addFolder, allConnections, folderOf, removeConnection, upsertConnection, validate } from "../src/tree";
import type { ConnectionDto } from "../src/api";

const conn = (id: string, extra: Partial<ConnectionDto> = {}): ConnectionDto => ({
  type: "connection", id, name: id, protocol: "ssh", host: `${id}.example.com`, username: "u", ...extra,
});

describe("tree editing", () => {
  it("adds to top level, existing folder, and creates missing folder", () => {
    let t: NodeDto[] = [{ type: "folder", name: "prod", children: [] }];
    t = upsertConnection(t, conn("a"), null);
    t = upsertConnection(t, conn("b"), "prod");
    t = upsertConnection(t, conn("c"), "new");
    expect(allConnections(t).map((c) => c.id).sort()).toEqual(["a", "b", "c"]);
    expect(folderOf(t, "b")).toBe("prod");
    expect(folderOf(t, "c")).toBe("new");
    expect(folderOf(t, "a")).toBeNull();
    expect(folderOf(t, "zzz")).toBeUndefined();
  });

  it("editing replaces in place and can move between folders without duplicating", () => {
    let t = upsertConnection([], conn("a"), "x");
    t = upsertConnection(t, conn("a", { host: "changed" }), "y");
    expect(allConnections(t)).toHaveLength(1);
    expect(allConnections(t)[0]!.host).toBe("changed");
    expect(folderOf(t, "a")).toBe("y");
  });

  it("editing keeps other hosts' jump references to it", () => {
    let t = upsertConnection([], conn("bastion"), null);
    t = upsertConnection(t, conn("db", { jump: ["bastion"] }), null);
    t = upsertConnection(t, conn("bastion", { name: "renamed" }), null);
    expect(allConnections(t).find((c) => c.id === "db")!.jump).toEqual(["bastion"]);
  });

  it("deleting removes the connection and dangling jump references", () => {
    let t = upsertConnection([], conn("bastion"), null);
    t = upsertConnection(t, conn("db", { jump: ["bastion"] }), null);
    t = removeConnection(t, "bastion");
    expect(allConnections(t).map((c) => c.id)).toEqual(["db"]);
    expect(allConnections(t)[0]!.jump).toEqual([]);
  });

  it("addFolder ignores blanks and duplicates", () => {
    const t = addFolder([], " prod ");
    expect(addFolder(t, "prod")).toBe(t);
    expect(addFolder(t, "  ")).toBe(t);
  });

  it("validates", () => {
    expect(validate(conn("a"))).toEqual({});
    expect(Object.keys(validate(conn("a", { name: " ", host: "" })))).toEqual(["name", "host"]);
    expect(validate(conn("a", { port: 70000 })).port).toBeDefined();
    expect(validate(conn("a", { port: 22 })).port).toBeUndefined();
    expect(validate(conn("a", { secret: { provider: "bitwarden", item: "", kind: "password" } })).secret).toBeDefined();
  });
});

describe("newId", () => {
  it("makes distinct v4 UUIDs", () => {
    const a = newId();
    expect(a).toMatch(/^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/);
    expect(newId()).not.toBe(a);
  });
});

import { exposedForwards, formatForwards, parseForwards } from "../src/tree";

describe("forward syntax", () => {
  it("parses ssh-like lines", () => {
    const r = parseForwards("L 5433:db.internal:5432\nr 9000:localhost:3000\nD 1080\nL 0.0.0.0:8080:web:80\n\nD 0.0.0.0:1081");
    expect(r.errors).toEqual([]);
    expect(r.forwards).toEqual([
      { kind: "local", bind_port: 5433, dest_host: "db.internal", dest_port: 5432 },
      { kind: "remote", bind_port: 9000, dest_host: "localhost", dest_port: 3000 },
      { kind: "dynamic", bind_port: 1080 },
      { kind: "local", bind_host: "0.0.0.0", bind_port: 8080, dest_host: "web", dest_port: 80 },
      { kind: "dynamic", bind_host: "0.0.0.0", bind_port: 1081 },
    ]);
  });

  it("reports errors with line numbers and keeps the good lines", () => {
    const r = parseForwards("L 1:h:2\nX 1\nL 99999:h:1\nD abc\nL 1:h");
    expect(r.forwards).toHaveLength(1);
    expect(r.errors.map((e) => e.split(":")[0])).toEqual(["line 2", "line 3", "line 4", "line 5"]);
  });

  it("round-trips through format", () => {
    const text = "L 5433:db.internal:5432\nR 9000:localhost:3000\nD 1080\nL 0.0.0.0:8080:web:80";
    expect(formatForwards(parseForwards(text).forwards)).toBe(text);
    expect(formatForwards(undefined)).toBe("");
  });

  it("flags non-loopback binds", () => {
    const f = parseForwards("L 1:h:2\nL 0.0.0.0:3:h:4\nD localhost:5").forwards;
    expect(exposedForwards(f)).toHaveLength(1);
  });
});
