//! Session manager: the application logic behind the UI commands, independent
//! of the GUI toolkit. The app shell implements [`Events`]; tests use a fake.

use crate::credentials::Credentials;
use crate::credentials::{CredentialError, Secret};
use crate::ext::{ExtensionHost, SessionMeta, SessionObserver};
use crate::hostkeys::KnownHosts;
use crate::model::{Folder, Node, Protocol, Tree};
use crate::redact::{SecretSet, StreamRedactor};
use crate::sftp::{Progress, Sftp};
use crate::ssh::{
    plan_hops, HopAuth, HostKeyPolicy, HostKeyPrompt, PlanError, Shell, ShellEvent, SshSession,
};
use crate::telnet::{TelnetEvent, TelnetSession, TelnetWriter};
use serde::Serialize;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use tokio::sync::{mpsc, oneshot};
use uuid::Uuid;

/// Notifications for the UI.
pub trait Events: Send + Sync + 'static {
    fn data(&self, session: &str, bytes: Vec<u8>);
    fn closed(&self, session: &str, reason: String);
    fn host_key_prompt(&self, prompt: PromptInfo);
    /// Ask the user for a password for `hop` (no vault secret is configured).
    fn credential_prompt(&self, prompt: CredentialPromptInfo);
    fn progress(&self, p: TransferInfo);
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PromptInfo {
    pub prompt_id: String,
    pub host: String,
    pub port: u16,
    pub key: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CredentialPromptInfo {
    pub prompt_id: String,
    /// `password` (for an SSH hop) or `unlock` (vault master password).
    pub kind: String,
    /// Set for `unlock`: which provider is being unlocked.
    pub provider: Option<String>,
    /// Set when a previous attempt failed (e.g. wrong master password).
    pub error: Option<String>,
    pub hop: String,
    pub username: String,
    pub host: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TransferInfo {
    pub transfer_id: String,
    pub done: u64,
    pub total: u64,
    pub finished: bool,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RemoteEntry {
    pub name: String,
    pub is_dir: bool,
    pub size: u64,
}

enum Cmd {
    Write(Vec<u8>),
    Resize(u32, u32),
    Close,
}

struct Active {
    tx: mpsc::UnboundedSender<Cmd>,
    ssh: Option<Arc<SshSession>>,
    sftp: Arc<tokio::sync::Mutex<Option<Arc<Sftp>>>>,
}

/// Feeds redacted output to extensions; the UI gets the raw bytes separately.
struct Tap {
    meta: SessionMeta,
    redactor: StreamRedactor,
    host: Arc<ExtensionHost>,
}

impl Tap {
    fn data(&mut self, raw: &[u8]) {
        let r = self.redactor.feed(raw);
        self.host.output(&self.meta, &r);
    }

    fn finish(&mut self) {
        let r = self.redactor.flush();
        self.host.output(&self.meta, &r);
        self.host.closed(&self.meta);
    }
}

pub struct Manager<E: Events> {
    events: Arc<E>,
    tree: Mutex<Tree>,
    tree_path: PathBuf,
    known_path: PathBuf,
    known: Arc<Mutex<KnownHosts>>,
    creds: Arc<Credentials>,
    sessions: Mutex<HashMap<String, Active>>,
    prompts: Arc<Mutex<HashMap<String, oneshot::Sender<bool>>>>,
    secret_prompts: Mutex<HashMap<String, oneshot::Sender<Option<String>>>>,
    next_id: AtomicU64,
    secrets: SecretSet,
    extensions: Arc<ExtensionHost>,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

impl<E: Events> Manager<E> {
    /// `config_dir` holds `connections.toml` and `known_hosts`.
    pub fn new(events: Arc<E>, config_dir: PathBuf, creds: Credentials) -> Result<Self, String> {
        std::fs::create_dir_all(&config_dir).map_err(|e| e.to_string())?;
        let tree_path = config_dir.join("connections.toml");
        let known_path = config_dir.join("known_hosts");
        Ok(Self {
            events,
            tree: Mutex::new(crate::store::load(&tree_path).map_err(|e| e.to_string())?),
            known: Arc::new(Mutex::new(
                KnownHosts::load(&known_path).map_err(|e| e.to_string())?,
            )),
            tree_path,
            known_path,
            creds: Arc::new(creds),
            sessions: Mutex::new(HashMap::new()),
            prompts: Arc::new(Mutex::new(HashMap::new())),
            secret_prompts: Mutex::new(HashMap::new()),
            next_id: AtomicU64::new(0),
            secrets: SecretSet::default(),
            extensions: Arc::new(ExtensionHost::default()),
        })
    }

    /// Register an extension that observes (redacted) session output.
    pub fn register_observer(&self, o: Arc<dyn SessionObserver>) {
        self.extensions.register(o);
    }

    pub fn tree(&self) -> Tree {
        lock(&self.tree).clone()
    }

    pub fn save_tree(&self, tree: Tree) -> Result<(), String> {
        crate::store::save(&self.tree_path, &tree).map_err(|e| e.to_string())?;
        *lock(&self.tree) = tree;
        Ok(())
    }

    /// Import hosts from an OpenSSH config file into a new folder; returns how many were added.
    pub fn import_ssh_config(&self, path: &std::path::Path) -> Result<usize, String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let user = std::env::var("USERNAME")
            .or_else(|_| std::env::var("USER"))
            .unwrap_or_default();
        let conns = crate::import::parse_ssh_config(&text, &user);
        let n = conns.len();
        if n == 0 {
            return Ok(0);
        }
        let mut tree = self.tree();
        tree.nodes.push(Node::Folder(Folder {
            name: "Imported from ssh_config".into(),
            children: conns.into_iter().map(Node::Connection).collect(),
        }));
        self.save_tree(tree)?;
        Ok(n)
    }

    pub fn answer_host_key(&self, prompt_id: &str, trust: bool) {
        if let Some(tx) = lock(&self.prompts).remove(prompt_id) {
            let _ = tx.send(trust);
        }
    }

    /// Answer a credential prompt; `None` cancels the connection attempt.
    pub fn answer_credential(&self, prompt_id: &str, secret: Option<String>) {
        if let Some(tx) = lock(&self.secret_prompts).remove(prompt_id) {
            let _ = tx.send(secret);
        }
    }

    async fn ask(&self, mut info: CredentialPromptInfo) -> Option<Secret> {
        let (tx, rx) = oneshot::channel();
        info.prompt_id = Uuid::new_v4().to_string();
        lock(&self.secret_prompts).insert(info.prompt_id.clone(), tx);
        self.events.credential_prompt(info);
        let secret = rx.await.ok().flatten().map(Secret::new);
        if let Some(s) = &secret {
            self.secrets.add(s);
        }
        secret
    }

    async fn ask_password(&self, hop: &crate::ssh::Hop) -> Result<Secret, String> {
        let info = CredentialPromptInfo {
            prompt_id: String::new(),
            kind: "password".into(),
            provider: None,
            error: None,
            hop: hop.name.clone(),
            username: hop.username.clone(),
            host: hop.host.clone(),
        };
        self.ask(info)
            .await
            .ok_or_else(|| format!("{}: password entry cancelled", hop.name))
    }

    /// Plan hops, unlocking a locked vault by asking for the master password (3 tries).
    async fn plan_with_unlock(
        &self,
        tree: &Tree,
        conn: &crate::model::Connection,
    ) -> Result<Vec<crate::ssh::Hop>, String> {
        let mut error = None;
        for _ in 0..4 {
            let (t, c, creds) = (tree.clone(), conn.clone(), self.creds.clone());
            let planned = tokio::task::spawn_blocking(move || plan_hops(&t, &c, &creds))
                .await
                .map_err(|e| e.to_string())?;
            let (hop, provider) = match planned {
                Ok(h) => return Ok(h),
                Err(PlanError::Credential {
                    hop,
                    provider,
                    source: CredentialError::Locked,
                }) => (hop, provider),
                Err(e) => return Err(e.to_string()),
            };
            let info = CredentialPromptInfo {
                prompt_id: String::new(),
                kind: "unlock".into(),
                provider: Some(provider.clone()),
                error: error.take(),
                hop,
                username: String::new(),
                host: String::new(),
            };
            let pw = self
                .ask(info)
                .await
                .ok_or_else(|| format!("{provider} is locked (unlock cancelled)"))?;
            let creds = self.creds.clone();
            let p = provider.clone();
            match tokio::task::spawn_blocking(move || creds.unlock(&p, &pw))
                .await
                .map_err(|e| e.to_string())?
            {
                Ok(()) => {}
                Err(CredentialError::BadPassword) => {
                    error = Some("Wrong master password".to_string())
                }
                Err(e) => return Err(e.to_string()),
            }
        }
        Err("too many failed unlock attempts".into())
    }

    fn policy(&self) -> HostKeyPolicy {
        let events = self.events.clone();
        let prompts = self.prompts.clone();
        Arc::new(move |p: HostKeyPrompt| {
            let events = events.clone();
            let prompts = prompts.clone();
            Box::pin(async move {
                let (tx, rx) = oneshot::channel();
                let prompt_id = Uuid::new_v4().to_string();
                lock(&prompts).insert(prompt_id.clone(), tx);
                events.host_key_prompt(PromptInfo {
                    prompt_id,
                    host: p.host,
                    port: p.port,
                    key: p.key,
                });
                rx.await.unwrap_or(false)
            })
        })
    }

    /// Open a session for a connection. Output and closing are reported via [`Events`].
    pub async fn open(
        &self,
        session_id: &str,
        connection_id: &str,
        cols: u32,
        rows: u32,
    ) -> Result<(), String> {
        let id = Uuid::parse_str(connection_id).map_err(|e| e.to_string())?;
        let tree = self.tree();
        let conn = tree.find(id).ok_or("unknown connection")?.clone();

        let (tx, mut rx) = mpsc::unbounded_channel::<Cmd>();
        let events = self.events.clone();
        let sid = session_id.to_string();
        let mut tap = Tap {
            meta: SessionMeta {
                id: sid.clone(),
                connection: conn.name.clone(),
                host: conn.host.clone(),
                username: conn.username.clone(),
            },
            redactor: StreamRedactor::new(self.secrets.clone()),
            host: self.extensions.clone(),
        };

        if conn.protocol == Protocol::Telnet {
            let mut t =
                TelnetSession::connect(&conn.host, conn.effective_port(), cols as u16, rows as u16)
                    .await
                    .map_err(|e| e.to_string())?;
            let w: TelnetWriter = t.writer();
            tokio::spawn(async move {
                loop {
                    tokio::select! {
                        ev = t.next() => match ev {
                            TelnetEvent::Data(d) => { tap.data(&d); events.data(&sid, d) }
                            TelnetEvent::Closed => { tap.finish(); events.closed(&sid, "connection closed".into()); break; }
                        },
                        cmd = rx.recv() => match cmd {
                            Some(Cmd::Write(b)) => { let _ = w.write(&b).await; }
                            Some(Cmd::Resize(c, r)) => { let _ = w.resize(c as u16, r as u16).await; }
                            Some(Cmd::Close) | None => { tap.finish(); break }
                        },
                    }
                }
            });
            lock(&self.sessions).insert(
                session_id.to_string(),
                Active {
                    tx,
                    ssh: None,
                    sftp: Default::default(),
                },
            );
            return Ok(());
        }

        // SSH, SFTP and SCP all ride on an SSH session (SCP is served via SFTP).
        let hops = self.plan_with_unlock(&tree, &conn).await?;
        let mut hops = hops;
        for hop in &mut hops {
            if matches!(hop.auth, HopAuth::None) {
                hop.auth = HopAuth::Password(self.ask_password(hop).await?);
            }
        }
        for hop in &hops {
            match &hop.auth {
                HopAuth::Password(p) => self.secrets.add(p),
                HopAuth::Key { pem, passphrase } => {
                    self.secrets.add(pem);
                    if let Some(p) = passphrase {
                        self.secrets.add(p);
                    }
                }
                HopAuth::None => {}
            }
        }
        let session = SshSession::connect(&hops, self.known.clone(), self.policy())
            .await
            .map_err(|e| e.to_string())?;
        let _ = lock(&self.known).save(&self.known_path);
        let session = Arc::new(session);
        let mut shell: Shell = session
            .open_shell(cols, rows)
            .await
            .map_err(|e| e.to_string())?;
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    ev = shell.next() => match ev {
                        ShellEvent::Data(d) => { tap.data(&d); events.data(&sid, d) }
                        ShellEvent::Exit(code) => { tap.finish(); events.closed(&sid, format!("exit status {code}")); break; }
                        ShellEvent::Closed => { tap.finish(); events.closed(&sid, "connection closed".into()); break; }
                    },
                    cmd = rx.recv() => match cmd {
                        Some(Cmd::Write(b)) => { let _ = shell.write(&b).await; }
                        Some(Cmd::Resize(c, r)) => { let _ = shell.resize(c, r).await; }
                        Some(Cmd::Close) | None => { tap.finish(); break }
                    },
                }
            }
        });
        lock(&self.sessions).insert(
            session_id.to_string(),
            Active {
                tx,
                ssh: Some(session),
                sftp: Default::default(),
            },
        );
        Ok(())
    }

    fn send(&self, session: &str, cmd: Cmd) {
        if let Some(a) = lock(&self.sessions).get(session) {
            let _ = a.tx.send(cmd);
        }
    }

    pub fn write(&self, session: &str, data: Vec<u8>) {
        self.send(session, Cmd::Write(data));
    }

    pub fn resize(&self, session: &str, cols: u32, rows: u32) {
        self.send(session, Cmd::Resize(cols, rows));
    }

    pub fn close(&self, session: &str) {
        if let Some(a) = lock(&self.sessions).remove(session) {
            let _ = a.tx.send(Cmd::Close);
        }
    }

    async fn sftp(&self, session: &str) -> Result<Arc<Sftp>, String> {
        let (ssh, cell) = {
            let s = lock(&self.sessions);
            let a = s.get(session).ok_or("no such session")?;
            (
                a.ssh.clone().ok_or("file transfer needs an SSH session")?,
                a.sftp.clone(),
            )
        };
        let mut guard = cell.lock().await;
        if let Some(s) = guard.as_ref() {
            return Ok(s.clone());
        }
        let s = Arc::new(ssh.open_sftp().await.map_err(|e| e.to_string())?);
        *guard = Some(s.clone());
        Ok(s)
    }

    pub async fn sftp_list(&self, session: &str, path: &str) -> Result<Vec<RemoteEntry>, String> {
        let sftp = self.sftp(session).await?;
        let path = if path == "." {
            sftp.home().await.map_err(|e| e.to_string())?
        } else {
            path.to_string()
        };
        Ok(sftp
            .list(&path)
            .await
            .map_err(|e| e.to_string())?
            .into_iter()
            .map(|e| RemoteEntry {
                name: e.name,
                is_dir: e.is_dir,
                size: e.size,
            })
            .collect())
    }

    fn start_transfer(
        &self,
        session: &str,
        upload: bool,
        local: PathBuf,
        remote: String,
    ) -> String {
        let transfer_id = format!("t{}", self.next_id.fetch_add(1, Ordering::SeqCst) + 1);
        let events = self.events.clone();
        let tid = transfer_id.clone();
        let sess = session.to_string();
        let this_sftp = {
            let s = lock(&self.sessions);
            s.get(&sess).map(|a| (a.ssh.clone(), a.sftp.clone()))
        };
        tokio::spawn(async move {
            let fail = |e: String| {
                events.progress(TransferInfo {
                    transfer_id: tid.clone(),
                    done: 0,
                    total: 0,
                    finished: true,
                    error: Some(e),
                })
            };
            let Some((Some(ssh), cell)) = this_sftp else {
                return fail("file transfer needs an SSH session".into());
            };
            let sftp = {
                let mut g = cell.lock().await;
                match g.as_ref() {
                    Some(s) => s.clone(),
                    None => match ssh.open_sftp().await {
                        Ok(s) => {
                            let s = Arc::new(s);
                            *g = Some(s.clone());
                            s
                        }
                        Err(e) => return fail(e.to_string()),
                    },
                }
            };
            let cancel = AtomicBool::new(false);
            let report = |p: Progress| {
                events.progress(TransferInfo {
                    transfer_id: tid.clone(),
                    done: p.done,
                    total: p.total,
                    finished: false,
                    error: None,
                })
            };
            let result = if upload {
                sftp.upload(&local, &remote, true, &cancel, report).await
            } else {
                sftp.download(&remote, &local, true, &cancel, report).await
            };
            match result {
                Ok(n) => events.progress(TransferInfo {
                    transfer_id: tid.clone(),
                    done: n,
                    total: n,
                    finished: true,
                    error: None,
                }),
                Err(e) => fail(e.to_string()),
            }
        });
        transfer_id
    }

    /// Upload a local file into `remote_dir`; returns the transfer id (progress via [`Events`]).
    pub async fn upload(
        &self,
        session: &str,
        local: &str,
        remote_dir: &str,
    ) -> Result<String, String> {
        let local = PathBuf::from(local);
        let name = local
            .file_name()
            .ok_or("bad file name")?
            .to_string_lossy()
            .to_string();
        let dir = if remote_dir == "." {
            self.sftp(session)
                .await?
                .home()
                .await
                .map_err(|e| e.to_string())?
        } else {
            remote_dir.to_string()
        };
        let remote = format!("{}/{}", dir.trim_end_matches('/'), name);
        Ok(self.start_transfer(session, true, local, remote))
    }

    pub async fn download(
        &self,
        session: &str,
        remote: &str,
        local_dir: &str,
    ) -> Result<String, String> {
        let name = remote.rsplit('/').next().unwrap_or(remote);
        let local = PathBuf::from(local_dir).join(name);
        Ok(self.start_transfer(session, false, local, remote.to_string()))
    }
}
