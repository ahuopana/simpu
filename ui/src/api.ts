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
}

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

export interface HostKeyPrompt {
  promptId: string;
  host: string;
  port: number;
  key: string;
}

export interface CredentialPrompt {
  promptId: string;
  /** "password" for an SSH hop, "unlock" for a vault master password. */
  kind: "password" | "unlock";
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

export type Unlisten = () => void;

export interface Backend {
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

  sftpList(sessionId: string, path: string): Promise<RemoteEntry[]>;
  upload(sessionId: string, localPath: string, remoteDir: string): Promise<string>;
  download(sessionId: string, remotePath: string, localDir: string): Promise<string>;
  onProgress(cb: (p: TransferProgress) => void): Unlisten;
}
