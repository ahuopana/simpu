import type { ConnectionDto, NodeDto } from "./api";

export function matches(c: ConnectionDto, q: string): boolean {
  const s = q.toLowerCase();
  return [c.name, c.host, c.username].some((v) => v.toLowerCase().includes(s));
}

/** Keep only connections matching `q` (and the folders that contain them). */
export function filterTree(nodes: NodeDto[], q: string): NodeDto[] {
  if (!q.trim()) return nodes;
  const out: NodeDto[] = [];
  for (const n of nodes) {
    if (n.type === "connection") {
      if (matches(n, q)) out.push(n);
    } else {
      const children = filterTree(n.children, q);
      if (children.length) out.push({ ...n, children });
    }
  }
  return out;
}

export function findConnection(nodes: NodeDto[], id: string): ConnectionDto | undefined {
  for (const n of nodes) {
    if (n.type === "connection" && n.id === id) return n;
    if (n.type === "folder") {
      const c = findConnection(n.children, id);
      if (c) return c;
    }
  }
}
