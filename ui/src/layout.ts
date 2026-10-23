// Pure layout model for tabs and split panes (no DOM), so it is unit-testable.

export type Dir = "row" | "col";

export type Pane =
  | { kind: "leaf"; id: string; sessionId: string }
  | { kind: "split"; dir: Dir; ratio: number; a: Pane; b: Pane };

export interface Tab {
  id: string;
  title: string;
  root: Pane;
  focus: string; // leaf id
}

export function leaves(p: Pane): Extract<Pane, { kind: "leaf" }>[] {
  return p.kind === "leaf" ? [p] : [...leaves(p.a), ...leaves(p.b)];
}

function mapLeaf(p: Pane, id: string, f: (leaf: Extract<Pane, { kind: "leaf" }>) => Pane): Pane {
  if (p.kind === "leaf") return p.id === id ? f(p) : p;
  return { ...p, a: mapLeaf(p.a, id, f), b: mapLeaf(p.b, id, f) };
}

/** Split `leafId`: the existing pane stays first, `fresh` is the new second pane. */
export function splitLeaf(tab: Tab, leafId: string, dir: Dir, fresh: { id: string; sessionId: string }): Tab {
  if (!leaves(tab.root).some((l) => l.id === leafId)) return tab;
  const root = mapLeaf(tab.root, leafId, (leaf) => ({
    kind: "split",
    dir,
    ratio: 0.5,
    a: leaf,
    b: { kind: "leaf", id: fresh.id, sessionId: fresh.sessionId },
  }));
  return { ...tab, root, focus: fresh.id };
}

function removeLeaf(p: Pane, id: string): Pane | null {
  if (p.kind === "leaf") return p.id === id ? null : p;
  const a = removeLeaf(p.a, id);
  const b = removeLeaf(p.b, id);
  if (a && b) return { ...p, a, b };
  return a ?? b; // sibling takes the parent's place
}

/** Close a pane. Returns null when it was the tab's last pane. */
export function closeLeaf(tab: Tab, leafId: string): Tab | null {
  const root = removeLeaf(tab.root, leafId);
  if (!root) return null;
  const remaining = leaves(root);
  const focus = remaining.some((l) => l.id === tab.focus) ? tab.focus : (remaining[0]?.id ?? "");
  return { ...tab, root, focus };
}

/** Set the divider ratio of the split whose first child contains `firstLeafId`. */
export function setRatio(tab: Tab, firstLeafId: string, ratio: number): Tab {
  const r = Math.min(0.9, Math.max(0.1, ratio));
  const go = (p: Pane): Pane => {
    if (p.kind === "leaf") return p;
    const direct = p.a.kind === "leaf" && p.a.id === firstLeafId;
    return { ...p, ratio: direct ? r : p.ratio, a: go(p.a), b: go(p.b) };
  };
  return { ...tab, root: go(tab.root) };
}

export function newTab(id: string, title: string, leaf: { id: string; sessionId: string }): Tab {
  return { id, title, root: { kind: "leaf", ...leaf }, focus: leaf.id };
}
