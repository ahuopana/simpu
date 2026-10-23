//! Thin Tauri shell around `simpu_core::manager::Manager`. Keep logic in core.

use serde::Serialize;
use simpu_core::credentials::Credentials;
use simpu_core::manager::{
    CredentialPromptInfo, Events, Manager, PromptInfo, RemoteEntry, TransferInfo,
};
use simpu_core::model::{Node, Tree};
use std::sync::Arc;
use tauri::{AppHandle, Emitter, Manager as _, State};

struct TauriEvents(AppHandle);

impl TauriEvents {
    fn emit<S: Serialize + Clone>(&self, name: &str, payload: S) {
        let _ = self.0.emit(name, payload);
    }
}

impl Events for TauriEvents {
    fn data(&self, session: &str, bytes: Vec<u8>) {
        self.emit(&format!("session-data-{session}"), bytes);
    }
    fn closed(&self, session: &str, reason: String) {
        self.emit(&format!("session-closed-{session}"), reason);
    }
    fn host_key_prompt(&self, p: PromptInfo) {
        self.emit("hostkey-prompt", p);
    }
    fn credential_prompt(&self, p: CredentialPromptInfo) {
        self.emit("credential-prompt", p);
    }
    fn progress(&self, p: TransferInfo) {
        self.emit("transfer-progress", p);
    }
}

type App = Arc<Manager<TauriEvents>>;

#[tauri::command]
fn tree_load(app: State<'_, App>) -> Vec<Node> {
    app.tree().nodes
}

#[tauri::command]
fn tree_save(app: State<'_, App>, nodes: Vec<Node>) -> Result<(), String> {
    app.save_tree(Tree { nodes })
}

#[tauri::command]
async fn session_open(
    app: State<'_, App>,
    session_id: String,
    connection_id: String,
    cols: u32,
    rows: u32,
) -> Result<(), String> {
    app.open(&session_id, &connection_id, cols, rows).await
}

#[tauri::command]
fn session_write(app: State<'_, App>, session_id: String, data: String) {
    app.write(&session_id, data.into_bytes());
}

#[tauri::command]
fn session_resize(app: State<'_, App>, session_id: String, cols: u32, rows: u32) {
    app.resize(&session_id, cols, rows);
}

#[tauri::command]
fn session_close(app: State<'_, App>, session_id: String) {
    app.close(&session_id);
}

#[tauri::command]
fn hostkey_answer(app: State<'_, App>, prompt_id: String, trust: bool) {
    app.answer_host_key(&prompt_id, trust);
}

#[tauri::command]
fn import_ssh_config(app: State<'_, App>) -> Result<usize, String> {
    let home = std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .ok_or("cannot find home directory")?;
    app.import_ssh_config(&std::path::PathBuf::from(home).join(".ssh").join("config"))
}

#[tauri::command]
fn credential_answer(app: State<'_, App>, prompt_id: String, secret: Option<String>) {
    app.answer_credential(&prompt_id, secret);
}

#[tauri::command]
async fn sftp_list(
    app: State<'_, App>,
    session_id: String,
    path: String,
) -> Result<Vec<RemoteEntry>, String> {
    app.sftp_list(&session_id, &path).await
}

#[tauri::command]
async fn sftp_upload(
    app: State<'_, App>,
    session_id: String,
    local_path: String,
    remote_dir: String,
) -> Result<String, String> {
    app.upload(&session_id, &local_path, &remote_dir).await
}

#[tauri::command]
async fn sftp_download(
    app: State<'_, App>,
    session_id: String,
    remote_path: String,
    local_dir: String,
) -> Result<String, String> {
    app.download(&session_id, &remote_path, &local_dir).await
}

pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            let dir = app.path().app_config_dir()?;
            let manager = Manager::new(
                Arc::new(TauriEvents(app.handle().clone())),
                dir,
                load_credentials(),
            )?;
            app.manage::<App>(Arc::new(manager));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            tree_load,
            tree_save,
            session_open,
            session_write,
            session_resize,
            session_close,
            hostkey_answer,
            credential_answer,
            import_ssh_config,
            sftp_list,
            sftp_upload,
            sftp_download
        ])
        .run(tauri::generate_context!())
        .expect("error while running Simpu");
}

/// Register credential providers. The Bitwarden plugin is started when its
/// executable sits next to the app and `BW_SESSION` is set (`bw unlock --raw`).
fn load_credentials() -> Credentials {
    let mut creds = Credentials::default();
    creds.register(Box::new(simpu_core::credentials::FileProvider::from_env()));
    if let (Ok(exe), Ok(session)) = (std::env::current_exe(), std::env::var("BW_SESSION")) {
        let name = if cfg!(windows) {
            "simpu-plugin-bitwarden.exe"
        } else {
            "simpu-plugin-bitwarden"
        };
        let plugin = exe.with_file_name(name);
        if plugin.exists() {
            match simpu_core::plugin::PluginProvider::spawn(
                &plugin,
                &[("BW_SESSION", session.as_str())],
            ) {
                Ok(p) => creds.register(Box::new(p)),
                Err(e) => eprintln!("bitwarden plugin failed to start: {e}"),
            }
        }
    }
    creds
}
