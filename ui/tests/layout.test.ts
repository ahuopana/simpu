import { describe, expect, it } from "vitest";
import { closeLeaf, leaves, newTab, setRatio, splitLeaf } from "../src/layout";

const base = () => newTab("t1", "db", { id: "p1", sessionId: "s1" });

describe("layout", () => {
  it("splits a leaf and focuses the new pane", () => {
    const t = splitLeaf(base(), "p1", "row", { id: "p2", sessionId: "s2" });
    expect(leaves(t.root).map((l) => l.id)).toEqual(["p1", "p2"]);
    expect(t.focus).toBe("p2");
  });

  it("splits nested panes", () => {
    let t = splitLeaf(base(), "p1", "row", { id: "p2", sessionId: "s2" });
    t = splitLeaf(t, "p2", "col", { id: "p3", sessionId: "s3" });
    expect(leaves(t.root).map((l) => l.id)).toEqual(["p1", "p2", "p3"]);
  });

  it("ignores unknown leaf", () => {
    const t = base();
    expect(splitLeaf(t, "nope", "row", { id: "x", sessionId: "y" })).toBe(t);
  });

  it("closing a pane promotes its sibling and fixes focus", () => {
    let t = splitLeaf(base(), "p1", "row", { id: "p2", sessionId: "s2" });
    t = splitLeaf(t, "p2", "col", { id: "p3", sessionId: "s3" });
    const c = closeLeaf(t, "p3")!;
    expect(leaves(c.root).map((l) => l.id)).toEqual(["p1", "p2"]);
    expect(c.focus).toBe("p1");
    const d = closeLeaf(c, "p1")!;
    expect(d.root.kind).toBe("leaf");
  });

  it("closing the last pane returns null", () => {
    expect(closeLeaf(base(), "p1")).toBeNull();
  });

  it("clamps ratio", () => {
    let t = splitLeaf(base(), "p1", "row", { id: "p2", sessionId: "s2" });
    t = setRatio(t, "p1", 0.99);
    expect(t.root.kind === "split" && t.root.ratio).toBe(0.9);
    t = setRatio(t, "p1", 0.01);
    expect(t.root.kind === "split" && t.root.ratio).toBe(0.1);
  });
});
