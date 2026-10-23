// In-memory backend used by `vite dev` in a plain browser and by the e2e tests.
import type { Backend, ConnectionDto, CredentialPrompt, HostKeyPrompt, NodeDto, RemoteEntry, TransferProgress } from "./api";

const sample: NodeDto[] = [
  {
    type: "folder",
    name: "prod",
    children: [
      { type: "connection", id: "db", name: "db", protocol: "ssh", host: "db.example.com", username: "root", jump: ["bastion"], secret: { provider: "bitwarden", item: "db-item", kind: "password" } },
      { type: "connection", id: "bastion", name: "bastion", protocol: "ssh", host: "bastion.example.com", username: "ops", secret: { provider: "bitwarden", item: "bastion-item", kind: "password" } },
    ],
  },
  { type: "connection", id: "adhoc", name: "adhoc", protocol: "ssh", host: "adhoc.example.com", username: "me" },
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
  private credPrompts = new Emitter<CredentialPrompt>();
  private pendingCred = new Map<string, (secret: string | null) => void>();
  /** Passwords entered by the user, for assertions in tests. */
  enteredPasswords: string[] = [];
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

  async importSshConfig() {
    this.tree.push({
      type: "folder",
      name: "Imported from ssh_config",
      children: [
        { type: "connection", id: "imp1", name: "imported-a", protocol: "ssh", host: "a.example.com", username: "me", secret: { provider: "file", item: "~/.ssh/id_ed25519", kind: "keyfile" } },
        { type: "connection", id: "imp2", name: "imported-b", protocol: "ssh", host: "b.example.com", username: "me", jump: ["imp1"] },
      ],
    });
    return 2;
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
    if (!conn.secret && conn.protocol !== "telnet") {
      const promptId = `c${this.n}`;
      const secret = await new Promise<string | null>((res) => {
        this.pendingCred.set(promptId, res);
        this.credPrompts.emit({ promptId, hop: conn.name, username: conn.username, host: conn.host });
      });
      if (secret === null) throw new Error(`${conn.name}: password entry cancelled`);
      this.enteredPasswords.push(secret);
    }
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
  onCredentialPrompt(cb: (p: CredentialPrompt) => void) {
    return this.credPrompts.on(cb);
  }
  async answerCredential(promptId: string, secret: string | null) {
    this.pendingCred.get(promptId)?.(secret);
    this.pendingCred.delete(promptId);
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
