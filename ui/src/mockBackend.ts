// In-memory backend used by `vite dev` in a plain browser and by the e2e tests.
import type { Backend, ForwardInfo, ForwardSpec, SessionState, CaptureStatus, ExportRequest, ExportResponse, ConnectionDto, CredentialPrompt, HostKeyPrompt, NodeDto, RemoteEntry, TransferProgress } from "./api";

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
  sessions = new Map<string, { conn: ConnectionDto; line: string }>();
  private hostPrompts = new Emitter<HostKeyPrompt>();
  private progress = new Emitter<TransferProgress>();
  private credPrompts = new Emitter<CredentialPrompt>();
  private pendingCred = new Map<string, (secret: string | null) => void>();
  /** Passwords entered by the user, for assertions in tests. */
  enteredPasswords: string[] = [];
  /** Mock Bitwarden: locked until the master password "master" is entered. */
  unlockedVaults = new Set<string>();
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

  // ----- forwards and reconnect (simplified stand-ins; real logic is tested in Rust) -----
  private stateEm = new Map<string, Emitter<SessionState>>();
  private fwdEm = new Map<string, Emitter<ForwardInfo[]>>();
  private forwards = new Map<string, ForwardInfo[]>();
  private nextFwd = 0;
  private lost = new Set<string>();
  /** How long the mock "auto reconnect" takes. */
  reconnectDelayMs = 200;
  /** Reconnects performed (for assertions). */
  reconnects: string[] = [];

  private setState(id: string, s: SessionState) {
    this.em(this.stateEm, id).emit(s);
  }
  private pushForwards(id: string) {
    this.em(this.fwdEm, id).emit(structuredClone(this.forwards.get(id) ?? []));
  }
  private listening(spec: ForwardSpec): ForwardInfo {
    const id = ++this.nextFwd;
    return { id, spec, connections: 0, state: "listening", port: spec.bind_port || 40000 + id };
  }
  onSessionState(id: string, cb: (s: SessionState) => void) {
    return this.em(this.stateEm, id).on(cb);
  }
  onForwards(id: string, cb: (l: ForwardInfo[]) => void) {
    return this.em(this.fwdEm, id).on(cb);
  }
  async forwardAdd(sessionId: string, spec: ForwardSpec) {
    if (!this.sessions.has(sessionId)) throw new Error("no such session");
    if (spec.kind !== "dynamic" && (!spec.dest_host || !spec.dest_port)) throw new Error("destination host and port are required");
    const info = this.listening(spec);
    this.forwards.set(sessionId, [...(this.forwards.get(sessionId) ?? []), info]);
    this.pushForwards(sessionId);
    return info;
  }
  async forwardStop(sessionId: string, forwardId: number) {
    this.forwards.set(sessionId, (this.forwards.get(sessionId) ?? []).filter((f) => f.id !== forwardId));
    this.pushForwards(sessionId);
  }
  async forwardList(sessionId: string) {
    return structuredClone(this.forwards.get(sessionId) ?? []);
  }
  /** Test hook: simulate the connection dropping. */
  dropSession(id: string) {
    const conn = this.sessions.get(id)?.conn;
    if (!conn) return;
    const mode = conn.reconnect ?? (conn.forwards?.length ? "auto" : "ask");
    this.out(id, "\r\n[connection lost]\r\n");
    if (mode === "off") {
      this.setState(id, { state: "closed", reason: "connection lost" });
      this.em(this.closed, id).emit("connection lost");
    } else if (mode === "ask") {
      this.lost.add(id);
      this.setState(id, { state: "lost", reason: "connection lost" });
    } else {
      this.setState(id, { state: "reconnecting", attempt: 1, nextInSecs: 1 });
      setTimeout(() => this.finishReconnect(id), this.reconnectDelayMs);
    }
  }
  private finishReconnect(id: string) {
    this.lost.delete(id);
    this.reconnects.push(id);
    this.out(id, "\r\n[reconnected]\r\n$ ");
    this.setState(id, { state: "connected" });
    this.pushForwards(id);
  }
  async reconnectSession(id: string) {
    if (!this.lost.has(id)) return;
    this.setState(id, { state: "reconnecting", attempt: 1, nextInSecs: 0 });
    setTimeout(() => this.finishReconnect(id), 100);
  }

  // ----- capture (simplified stand-in for simpu-capture; the real logic is unit-tested in Rust) -----
  private logs = new Map<string, { recording: boolean; text: string; user: string; host: string }>();
  savedFiles: { path: string; text: string }[] = [];
  clipboard = "";
  nextSavePath: string | null = "/tmp/session.log";

  private tap(sessionId: string, s: string) {
    const l = this.logs.get(sessionId);
    if (l?.recording) l.text += s;
  }
  async captureStart(sessionId: string) {
    const c = this.sessions.get(sessionId)?.conn;
    this.logs.set(sessionId, { recording: true, text: "", user: c?.username ?? "", host: c?.host ?? "" });
  }
  async captureStop(sessionId: string) {
    const l = this.logs.get(sessionId);
    if (l) l.recording = false;
  }
  async captureStatus(sessionId: string): Promise<CaptureStatus> {
    const l = this.logs.get(sessionId);
    return { recording: l?.recording ?? false, bytes: l?.text.length ?? 0 };
  }
  async capturePreview(sessionId: string, req: ExportRequest): Promise<ExportResponse> {
    const l = this.logs.get(sessionId);
    if (!l) throw new Error(`no capture for session ${sessionId}`);
    let text = l.text.replace(/\x1b\[[0-9;]*[A-Za-z]/g, "").replace(/\r/g, "");
    let fallback = false;
    if (req.scope === "last") {
      const blocks = text.split(/^(?=\S*[$#] )/m).filter((b) => /^\S*[$#] \S/.test(b));
      fallback = blocks.length === 0;
      text = (fallback ? text.trimEnd().split("\n").slice(-req.lastN) : blocks.slice(-req.lastN)).join(fallback ? "\n" : "").trimEnd();
    }
    const mapping: [string, string][] = [];
    const a = req.anonymize;
    if (a) {
      const swap = (orig: string, pseudo: string) => {
        if (orig && text.includes(orig)) {
          text = text.split(orig).join(pseudo);
          mapping.push([orig, pseudo]);
        }
      };
      if (a.usernames) swap(l.user, "user1");
      if (a.hostnames) swap(l.host, "host1");
      if (a.secrets) text = text.replace(/(password\s*[=:]\s*)\S+/gi, "$1******");
      a.custom.filter(Boolean).forEach((p, i) => {
        text = text.replace(new RegExp(p, "g"), (m) => {
          mapping.push([m, `<rule${i + 1}-1>`]);
          return `<rule${i + 1}-1>`;
        });
      });
    }
    return { text, mapping, fallbackToLines: fallback };
  }
  async captureSave(sessionId: string, req: ExportRequest, path: string) {
    this.savedFiles.push({ path, text: (await this.capturePreview(sessionId, req)).text });
  }
  async captureRegexError(pattern: string) {
    try {
      new RegExp(pattern);
      return null;
    } catch (e) {
      return String(e instanceof Error ? e.message : e);
    }
  }
  async pickSavePath() {
    return this.nextSavePath;
  }
  async copyText(text: string) {
    this.clipboard = text;
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
    this.tap(id, s);
    this.em(this.data, id).emit(new TextEncoder().encode(s));
  }

  async openSession(id: string, connectionId: string) {
    const conn = findConn(this.tree, connectionId);
    if (!conn) throw new Error("unknown connection");
    this.n++;
    let unlockError: string | null = null;
    const provider = conn.secret?.provider ?? "";
    while (["bitwarden", "keepassxc"].includes(provider) && !this.unlockedVaults.has(provider)) {
      const promptId = `u${++this.n}`;
      const pw = await new Promise<string | null>((res) => {
        this.pendingCred.set(promptId, res);
        this.credPrompts.emit({ promptId, kind: "unlock", provider, error: unlockError, hop: conn.name, username: "", host: "" });
      });
      if (pw === null) throw new Error(`${provider} is locked (unlock cancelled)`);
      if (pw === "master") this.unlockedVaults.add(provider);
      else unlockError = "Wrong master password";
    }
    if (!conn.secret && conn.protocol !== "telnet") {
      const promptId = `c${this.n}`;
      const secret = await new Promise<string | null>((res) => {
        this.pendingCred.set(promptId, res);
        this.credPrompts.emit({ promptId, kind: "password", hop: conn.name, username: conn.username, host: conn.host });
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
    this.setState(id, { state: "connected" });
    this.forwards.set(id, (conn.forwards ?? []).map((f) => this.listening(f)));
    this.pushForwards(id);
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
