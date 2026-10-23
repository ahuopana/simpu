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
