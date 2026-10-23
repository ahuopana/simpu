// In-memory backend used by `vite dev` in a plain browser and by the e2e tests.
import type { Backend, ConnectionDto, HostKeyPrompt, NodeDto, RemoteEntry, TransferProgress } from "./api";

const sample: NodeDto[] = [
  {
    type: "folder",
    name: "prod",
    children: [
      { type: "connection", id: "db", name: "db", protocol: "ssh", host: "db.example.com", username: "root", jump: ["bastion"] },
      { type: "connection", id: "bastion", name: "bastion", protocol: "ssh", host: "bastion.example.com", username: "ops" },
    ],
  },
  { type: "connection", id: "sw", name: "switch", protocol: "telnet", host: "10.0.0.1", username: "admin" },
];

type Listener<T> = (v: T) => void;

class Emitter<T> {
  private ls = new Set<Listener<T>>();
  on(l: Listener<T>) {
    this.ls.add(l);
    return () => void this.ls.delete(l);
  }
  emit(v: T) {
    this.ls.forEach((l) => l(v));
  }
}

function findConn(nodes: NodeDto[], id: string): ConnectionDto | undefined {
  for (const n of nodes) {
    if (n.type === "connection" && n.id === id) return n;
    if (n.type === "folder") {
      const c = findConn(n.children, id);
      if (c) return c;
    }
  }
}

export class MockBackend implements Backend {
  tree: NodeDto[] = structuredClone(sample);
  private data = new Map<string, Emitter<Uint8Array>>();
  private closed = new Map<string, Emitter<string>>();
  private sessions = new Map<string, { conn: ConnectionDto; line: string }>();
  private hostPrompts = new Emitter<HostKeyPrompt>();
  private progress = new Emitter<TransferProgress>();
  private pendingPrompt = new Map<string, (trust: boolean) => void>();
  private n = 0;
  /** Connections to which the user has already trusted the host key. */
  trusted = new Set<string>();
  uploads: { sessionId: string; localPath: string; remoteDir: string }[] = [];

  async loadTree() {
    return structuredClone(this.tree);
  }
  async saveTree(nodes: NodeDto[]) {
    this.tree = structuredClone(nodes);
  }

  private em<T>(m: Map<string, Emitter<T>>, id: string) {
    let e = m.get(id);
    if (!e) m.set(id, (e = new Emitter<T>()));
    return e;
  }

  private out(id: string, s: string) {
    this.em(this.data, id).emit(new TextEncoder().encode(s));
  }

  async openSession(id: string, connectionId: string) {
    const conn = findConn(this.tree, connectionId);
    if (!conn) throw new Error("unknown connection");
    this.n++;
    if (!this.trusted.has(conn.host)) {
      const promptId = `p${this.n}`;
      const ok = await new Promise<boolean>((res) => {
        this.pendingPrompt.set(promptId, res);
        this.hostPrompts.emit({ promptId, host: conn.host, port: conn.port ?? 22, key: "ssh-ed25519 AAAA…" });
      });
      if (!ok) throw new Error("host key rejected by user");
      this.trusted.add(conn.host);
    }
    this.sessions.set(id, { conn, line: "" });
    this.out(id, `Connected to ${conn.username}@${conn.host}\r\n$ `);
  }

  async write(sessionId: string, data: string) {
    const s = this.sessions.get(sessionId);
    if (!s) return;
    for (const ch of data) {
      if (ch === "\r") {
        const cmd = s.line;
        s.line = "";
        if (cmd === "exit") {
          this.out(sessionId, "\r\nlogout\r\n");
          this.em(this.closed, sessionId).emit("exit status 0");
          return;
        }
        this.out(sessionId, `\r\nran: ${cmd}\r\n$ `);
      } else {
        s.line += ch;
        this.out(sessionId, ch);
      }
    }
  }
  async resize() {}
  async closeSession(sessionId: string) {
    this.sessions.delete(sessionId);
  }
  onData(sessionId: string, cb: (b: Uint8Array) => void) {
    return this.em(this.data, sessionId).on(cb);
  }
  onClosed(sessionId: string, cb: (r: string) => void) {
    return this.em(this.closed, sessionId).on(cb);
  }
  onHostKeyPrompt(cb: (p: HostKeyPrompt) => void) {
    return this.hostPrompts.on(cb);
  }
  async answerHostKey(promptId: string, trust: boolean) {
    this.pendingPrompt.get(promptId)?.(trust);
    this.pendingPrompt.delete(promptId);
  }
  async sftpList(): Promise<RemoteEntry[]> {
    return [
      { name: "logs", isDir: true, size: 0 },
      { name: "notes.txt", isDir: false, size: 12 },
    ];
  }
  async upload(sessionId: string, localPath: string, remoteDir: string) {
    this.uploads.push({ sessionId, localPath, remoteDir });
    const transferId = `t${++this.n}`;
    queueMicrotask(() => {
      this.progress.emit({ transferId, done: 5, total: 10, finished: false });
      this.progress.emit({ transferId, done: 10, total: 10, finished: true });
    });
    return transferId;
  }
  async download() {
    return `t${++this.n}`;
  }
  onProgress(cb: (p: TransferProgress) => void) {
    return this.progress.on(cb);
  }
}
