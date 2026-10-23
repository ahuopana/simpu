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
