// Backend contract. Implemented by tauriBackend (real) and mockBackend (dev/tests).

export type Protocol = "ssh" | "telnet" | "sftp" | "scp";

export interface ConnectionDto {
  type: "connection";
  id: string;
  name: string;
  protocol: Protocol;
  host: string;
  port?: number;
  username: string;
  jump?: string[];
  secret?: { provider: string; item: string; kind: "password" | "keyfile" };
  forwards?: ForwardSpec[];
  /** Omitted = default: auto when the connection has forwards, otherwise ask. */
  reconnect?: "auto" | "ask" | "off";
  /** Tunnel only: no interactive shell. */
  no_shell?: boolean;
}

export interface ForwardSpec {
  kind: "local" | "remote" | "dynamic";
  bind_host?: string; // default 127.0.0.1
  bind_port: number;
  dest_host?: string;
  dest_port?: number;
}

export type ForwardInfo = {
  id: number;
  spec: ForwardSpec;
  connections: number;
} & ({ state: "listening"; port: number } | { state: "failed"; reason: string });

export type SessionState =
  | { state: "connecting" }
  | { state: "connected" }
  | { state: "reconnecting"; attempt: number; nextInSecs: number }
  | { state: "lost"; reason: string }
  | { state: "closed"; reason: string };

export interface FolderDto {
  type: "folder";
  name: string;
  children: NodeDto[];
}

export type NodeDto = ConnectionDto | FolderDto;

export interface RemoteEntry {
  name: string;
  isDir: boolean;
  size: number;
}

export interface RemoteDir {
  /** Canonical absolute path. */
  path: string;
  entries: RemoteEntry[];
}

export interface HostKeyPrompt {
  promptId: string;
  host: string;
  port: number;
  key: string;
}

export interface CredentialPrompt {
  promptId: string;
  /** "password" for an SSH hop, "unlock" for a vault master password, "passphrase" for an encrypted key. */
  kind: "password" | "unlock" | "passphrase";
  provider?: string | null;
  error?: string | null;
  hop: string;
  username: string;
  host: string;
}

export interface TransferProgress {
  transferId: string;
  done: number;
  total: number;
  finished: boolean;
  error?: string;
}

export interface AnonymizeRequest {
  usernames: boolean;
  hostnames: boolean;
  secrets: boolean;
  /** One regular expression per entry. */
  custom: string[];
}

export interface ExportRequest {
  scope: "all" | "last";
  lastN: number;
  stripEscapes: boolean;
  promptRegex?: string;
  anonymize?: AnonymizeRequest;
}

export interface ExportResponse {
  text: string;
  /** [original, pseudonym] pairs. */
  mapping: [string, string][];
  fallbackToLines: boolean;
}

export interface CaptureStatus {
  recording: boolean;
  bytes: number;
}

export interface Settings {
  keepassxc: { database: string; keyfile: string; cli: string };
  bitwarden: { cli: string };
  network: { keepalive_secs: number; keepalive_max: number; connect_timeout_secs: number };
  reconnect: { base_secs: number; max_secs: number; max_attempts: number };
}

export type Unlisten = () => void;

export interface Backend {
  getSettings(): Promise<Settings>;
  /** Rejects with a message listing the problems when the settings are not valid. */
  saveSettings(settings: Settings): Promise<void>;
  loadTree(): Promise<NodeDto[]>;
  saveTree(nodes: NodeDto[]): Promise<void>;
  /** Import ~/.ssh/config into a new folder; resolves to the number of hosts added. */
  importSshConfig(): Promise<number>;

  /** The caller picks `sessionId` and subscribes to onData/onClosed *before* calling, so no output is missed. */
  openSession(sessionId: string, connectionId: string, cols: number, rows: number): Promise<void>;
  write(sessionId: string, data: string): Promise<void>;
  resize(sessionId: string, cols: number, rows: number): Promise<void>;
  closeSession(sessionId: string): Promise<void>;
  onData(sessionId: string, cb: (bytes: Uint8Array) => void): Unlisten;
  onClosed(sessionId: string, cb: (reason: string) => void): Unlisten;

  onHostKeyPrompt(cb: (p: HostKeyPrompt) => void): Unlisten;
  answerHostKey(promptId: string, trust: boolean): Promise<void>;

  onCredentialPrompt(cb: (p: CredentialPrompt) => void): Unlisten;
  /** `null` cancels the connection attempt. */
  answerCredential(promptId: string, secret: string | null): Promise<void>;

  onSessionState(sessionId: string, cb: (s: SessionState) => void): Unlisten;
  onForwards(sessionId: string, cb: (list: ForwardInfo[]) => void): Unlisten;
  reconnectSession(sessionId: string): Promise<void>;
  forwardAdd(sessionId: string, spec: ForwardSpec): Promise<ForwardInfo>;
  forwardStop(sessionId: string, forwardId: number): Promise<void>;
  forwardList(sessionId: string): Promise<ForwardInfo[]>;

  captureStart(sessionId: string): Promise<void>;
  captureStop(sessionId: string): Promise<void>;
  captureStatus(sessionId: string): Promise<CaptureStatus>;
  capturePreview(sessionId: string, request: ExportRequest): Promise<ExportResponse>;
  captureSave(sessionId: string, request: ExportRequest, path: string): Promise<void>;
  /** Error message if `pattern` is not a valid regular expression, otherwise null. */
  captureRegexError(pattern: string): Promise<string | null>;
  /** Ask the user where to save a file; null if cancelled. */
  pickSavePath(defaultName: string): Promise<string | null>;
  copyText(text: string): Promise<void>;

  /** List a directory (`.` = home); the result carries the canonical absolute path. */
  sftpDir(sessionId: string, path: string): Promise<RemoteDir>;
  sftpMkdir(sessionId: string, path: string): Promise<void>;
  sftpRename(sessionId: string, from: string, to: string): Promise<void>;
  sftpRemove(sessionId: string, path: string, isDir: boolean): Promise<void>;
  /** Ask the user for a local folder (download target); null if cancelled. */
  pickFolder(): Promise<string | null>;
  /** Ask the user for local files to upload; empty if cancelled. */
  pickFiles(): Promise<string[]>;
  sftpList(sessionId: string, path: string): Promise<RemoteEntry[]>;
  upload(sessionId: string, localPath: string, remoteDir: string): Promise<string>;
  download(sessionId: string, remotePath: string, localDir: string): Promise<string>;
  onProgress(cb: (p: TransferProgress) => void): Unlisten;
}
