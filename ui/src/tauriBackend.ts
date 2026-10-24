// Real backend: forwards to the Rust commands in crates/simpu-app.
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { save } from "@tauri-apps/plugin-dialog";
import type { Backend, CredentialPrompt, ForwardInfo, HostKeyPrompt, SessionState, TransferProgress, Unlisten } from "./api";

function sub<T>(event: string, cb: (p: T) => void): Unlisten {
  let un: (() => void) | undefined;
  let cancelled = false;
  void listen<T>(event, (e) => cb(e.payload)).then((u) => (cancelled ? u() : (un = u)));
  return () => {
    cancelled = true;
    un?.();
  };
}

export const tauriBackend: Backend = {
  loadTree: () => invoke("tree_load"),
  saveTree: (nodes) => invoke("tree_save", { nodes }),
  importSshConfig: () => invoke("import_ssh_config"),
  openSession: (sessionId, connectionId, cols, rows) => invoke("session_open", { sessionId, connectionId, cols, rows }),
  write: (sessionId, data) => invoke("session_write", { sessionId, data }),
  resize: (sessionId, cols, rows) => invoke("session_resize", { sessionId, cols, rows }),
  closeSession: (sessionId) => invoke("session_close", { sessionId }),
  onData: (sessionId, cb) => sub<number[]>(`session-data-${sessionId}`, (a) => cb(new Uint8Array(a))),
  onClosed: (sessionId, cb) => sub<string>(`session-closed-${sessionId}`, cb),
  onHostKeyPrompt: (cb) => sub<HostKeyPrompt>("hostkey-prompt", cb),
  answerHostKey: (promptId, trust) => invoke("hostkey_answer", { promptId, trust }),
  onCredentialPrompt: (cb) => sub<CredentialPrompt>("credential-prompt", cb),
  answerCredential: (promptId, secret) => invoke("credential_answer", { promptId, secret }),
  onSessionState: (sessionId, cb) => sub<SessionState>(`session-state-${sessionId}`, cb),
  onForwards: (sessionId, cb) => sub<ForwardInfo[]>(`forwards-${sessionId}`, cb),
  reconnectSession: (sessionId) => invoke("session_reconnect", { sessionId }),
  forwardAdd: (sessionId, spec) => invoke("forward_add", { sessionId, spec }),
  forwardStop: (sessionId, forwardId) => invoke("forward_stop", { sessionId, forwardId }),
  forwardList: (sessionId) => invoke("forward_list", { sessionId }),
  captureStart: (sessionId) => invoke("capture_start", { sessionId }),
  captureStop: (sessionId) => invoke("capture_stop", { sessionId }),
  captureStatus: (sessionId) => invoke("capture_status", { sessionId }),
  capturePreview: (sessionId, request) => invoke("capture_preview", { sessionId, request }),
  captureSave: (sessionId, request, path) => invoke("capture_save", { sessionId, request, path }),
  captureRegexError: (pattern) => invoke("capture_regex_error", { pattern }),
  pickSavePath: (defaultName) => save({ defaultPath: defaultName }),
  copyText: (text) => navigator.clipboard.writeText(text),
  sftpList: (sessionId, path) => invoke("sftp_list", { sessionId, path }),
  upload: (sessionId, localPath, remoteDir) => invoke("sftp_upload", { sessionId, localPath, remoteDir }),
  download: (sessionId, remotePath, localDir) => invoke("sftp_download", { sessionId, remotePath, localDir }),
  onProgress: (cb) => sub<TransferProgress>("transfer-progress", cb),
};
