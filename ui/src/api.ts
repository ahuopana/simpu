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

export interface TransferProgress {
  transferId: string;
  done: number;
  total: number;
  finished: boolean;
  error?: string;
}

export type Unlisten = () => void;

export interface Backend {
  loadTree(): Promise<NodeDto[]>;
  saveTree(nodes: NodeDto[]): Promise<void>;

  /** The caller picks `sessionId` and subscribes to onData/onClosed *before* calling, so no output is missed. */
  openSession(sessionId: string, connectionId: string, cols: number, rows: number): Promise<void>;
  write(sessionId: string, data: string): Promise<void>;
  resize(sessionId: string, cols: number, rows: number): Promise<void>;
  closeSession(sessionId: string): Promise<void>;
  onData(sessionId: string, cb: (bytes: Uint8Array) => void): Unlisten;
  onClosed(sessionId: string, cb: (reason: string) => void): Unlisten;

  onHostKeyPrompt(cb: (p: HostKeyPrompt) => void): Unlisten;
  answerHostKey(promptId: string, trust: boolean): Promise<void>;

  sftpList(sessionId: string, path: string): Promise<RemoteEntry[]>;
  upload(sessionId: string, localPath: string, remoteDir: string): Promise<string>;
  download(sessionId: string, remotePath: string, localDir: string): Promise<string>;
  onProgress(cb: (p: TransferProgress) => void): Unlisten;
}
