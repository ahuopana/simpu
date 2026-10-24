//! Thin Tauri shell around `simpu_core::manager::Manager`. Keep logic in core.

use serde::Serialize;
use simpu_capture::api::{ExportRequest, ExportResponse, KnownNames, StatusResponse};
use simpu_capture::Capture;
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
type Cap = Arc<Capture>;

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
fn session_close(app: State<'_, App>, cap: State<'_, Cap>, session_id: String) {
    app.close(&session_id);
    cap.discard(&session_id);
}

#[tauri::command]
fn capture_start(cap: State<'_, Cap>, session_id: String) {
    cap.start(&session_id);
}

#[tauri::command]
fn capture_stop(cap: State<'_, Cap>, session_id: String) {
    cap.stop(&session_id);
}

#[tauri::command]
fn capture_status(cap: State<'_, Cap>, session_id: String) -> StatusResponse {
    cap.status_response(&session_id)
}

/// Usernames and hosts from the connection tree, so they are anonymized everywhere.
fn known_names(app: &App) -> KnownNames {
    let tree = app.tree();
    let conns = tree.connections();
    KnownNames {
        usernames: conns
            .iter()
            .map(|c| c.username.clone())
            .filter(|u| !u.is_empty())
            .collect(),
        hostnames: conns.iter().map(|c| c.host.clone()).collect(),
    }
}

#[tauri::command]
fn capture_preview(
    app: State<'_, App>,
    cap: State<'_, Cap>,
    session_id: String,
    request: ExportRequest,
) -> Result<ExportResponse, String> {
    cap.export_request(&session_id, request, &known_names(&app))
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn capture_save(
    app: State<'_, App>,
    cap: State<'_, Cap>,
    session_id: String,
    request: ExportRequest,
    path: String,
) -> Result<(), String> {
    let r = cap
        .export_request(&session_id, request, &known_names(&app))
        .map_err(|e| e.to_string())?;
    simpu_capture::api::write_atomic(std::path::Path::new(&path), &r.text)
        .map_err(|e| format!("{path}: {e}"))
}

#[tauri::command]
fn capture_regex_error(pattern: String) -> Option<String> {
    simpu_capture::api::regex_error(&pattern)
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
            capture_start,
            capture_stop,
            capture_status,
            capture_preview,
            capture_save,
            capture_regex_error,
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

/// Start a plugin executable that sits next to the app, if it exists.
fn spawn_plugin(creds: &mut Credentials, file: &str, env: &[(&str, &str)]) {
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    let name = if cfg!(windows) {
        format!("{file}.exe")
    } else {
        file.to_string()
    };
    let plugin = exe.with_file_name(name);
    if !plugin.exists() {
        return;
    }
    match simpu_core::plugin::PluginProvider::spawn(&plugin, env) {
        Ok(p) => creds.register(Box::new(p)),
        Err(e) => eprintln!("plugin {file} failed to start: {e}"),
    }
}

/// Register credential providers: local key files always; Bitwarden and
/// KeePassXC when their plugin executable is installed next to the app.
/// Bitwarden unlocks in-app (or inherits `BW_SESSION`); KeePassXC needs
/// `KEEPASSXC_DB` (and optionally `KEEPASSXC_KEYFILE`, `KEEPASSXC_CLI`).
fn load_credentials() -> Credentials {
    let mut creds = Credentials::default();
    creds.register(Box::new(simpu_core::credentials::FileProvider::from_env()));
    let bw_session = std::env::var("BW_SESSION").unwrap_or_default();
    spawn_plugin(
        &mut creds,
        "simpu-plugin-bitwarden",
        &[("BW_SESSION", bw_session.as_str())],
    );
    if let Ok(db) = std::env::var("KEEPASSXC_DB") {
        let key = std::env::var("KEEPASSXC_KEYFILE").unwrap_or_default();
        let cli = std::env::var("KEEPASSXC_CLI").unwrap_or_else(|_| "keepassxc-cli".into());
        spawn_plugin(
            &mut creds,
            "simpu-plugin-keepassxc",
            &[
                ("KEEPASSXC_DB", db.as_str()),
                ("KEEPASSXC_KEYFILE", key.as_str()),
                ("KEEPASSXC_CLI", cli.as_str()),
            ],
        );
    }
    creds
}
