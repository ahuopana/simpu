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

// ---------- editing (pure) ----------

export function allConnections(nodes: NodeDto[]): ConnectionDto[] {
  return nodes.flatMap((n) => (n.type === "connection" ? [n] : allConnections(n.children)));
}

export function folderNames(nodes: NodeDto[]): string[] {
  return nodes.flatMap((n) => (n.type === "folder" ? [n.name, ...folderNames(n.children)] : []));
}

function mapFolder(nodes: NodeDto[], name: string, f: (children: NodeDto[]) => NodeDto[]): NodeDto[] {
  return nodes.map((n) => {
    if (n.type !== "folder") return n;
    if (n.name === name) return { ...n, children: f(n.children) };
    return { ...n, children: mapFolder(n.children, name, f) };
  });
}

/** Remove a connection from wherever it lives (and from every jump list). */
export function removeConnection(nodes: NodeDto[], id: string): NodeDto[] {
  const out: NodeDto[] = [];
  for (const n of nodes) {
    if (n.type === "connection") {
      if (n.id === id) continue;
      out.push(n.jump?.includes(id) ? { ...n, jump: n.jump.filter((j) => j !== id) } : n);
    } else out.push({ ...n, children: removeConnection(n.children, id) });
  }
  return out;
}

function stripNode(nodes: NodeDto[], id: string): NodeDto[] {
  return nodes
    .filter((n) => !(n.type === "connection" && n.id === id))
    .map((n) => (n.type === "folder" ? { ...n, children: stripNode(n.children, id) } : n));
}

/** Insert or replace `conn`; `folder` is the destination folder name (null = top level). */
export function upsertConnection(nodes: NodeDto[], conn: ConnectionDto, folder: string | null): NodeDto[] {
  const kept = stripNode(nodes, conn.id);
  if (folder === null) return [...kept, conn];
  if (!folderNames(kept).includes(folder)) return [...kept, { type: "folder", name: folder, children: [conn] }];
  return mapFolder(kept, folder, (c) => [...c, conn]);
}

export function addFolder(nodes: NodeDto[], name: string): NodeDto[] {
  const n = name.trim();
  if (!n || folderNames(nodes).includes(n)) return nodes;
  return [...nodes, { type: "folder", name: n, children: [] }];
}

export function folderOf(nodes: NodeDto[], id: string, current: string | null = null): string | null | undefined {
  for (const n of nodes) {
    if (n.type === "connection" && n.id === id) return current;
    if (n.type === "folder") {
      const r = folderOf(n.children, id, n.name);
      if (r !== undefined) return r;
    }
  }
  return undefined;
}

export interface FormErrors {
  [field: string]: string;
}

export function validate(c: ConnectionDto): FormErrors {
  const e: FormErrors = {};
  if (!c.name.trim()) e.name = "Name is required";
  if (!c.host.trim()) e.host = "Host is required";
  if (c.port !== undefined && (!Number.isInteger(c.port) || c.port < 1 || c.port > 65535)) e.port = "Port must be 1-65535";
  if (c.secret && !c.secret.item.trim()) e.secret = "Secret item is required when a provider is set";
  return e;
}

/** RFC 4122 v4 UUID; does not rely on a secure context like crypto.randomUUID does. */
export function newId(): string {
  const b = crypto.getRandomValues(new Uint8Array(16));
  b[6] = (b[6]! & 0x0f) | 0x40;
  b[8] = (b[8]! & 0x3f) | 0x80;
  const h = Array.from(b, (x) => x.toString(16).padStart(2, "0")).join("");
  return `${h.slice(0, 8)}-${h.slice(8, 12)}-${h.slice(12, 16)}-${h.slice(16, 20)}-${h.slice(20)}`;
}
