//! Session manager: the application logic behind the UI commands, independent
//! of the GUI toolkit. The app shell implements [`Events`]; tests use a fake.

use crate::credentials::Credentials;
use crate::credentials::{CredentialError, Secret};
use crate::ext::{ExtensionHost, SessionMeta, SessionObserver};
use crate::forward::{ForwardInfo, ForwardManager, ForwardState};
use crate::hostkeys::KnownHosts;
use crate::model::{Folder, Forward, Node, Protocol, ReconnectPolicy, Tree};
use crate::redact::{SecretSet, StreamRedactor};
use crate::sftp::{Progress, Sftp};
use crate::ssh::{
    plan_hops, ConnectOptions, ErrorKind, Hop, HopAuth, HostKeyPolicy, HostKeyPrompt, PlanError,
    Shell, ShellEvent, SshError, SshSession,
};
use crate::telnet::{TelnetEvent, TelnetSession, TelnetWriter};
use serde::Serialize;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
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
    /// Connection state changes (connected, reconnecting, lost, closed).
    fn session_state(&self, _session: &str, _state: SessionState) {}
    /// The set of forwards of a session changed (started, stopped, restored after reconnect).
    fn forwards(&self, _session: &str, _list: Vec<ForwardInfo>) {}
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(
    tag = "state",
    rename_all = "lowercase",
    rename_all_fields = "camelCase"
)]
pub enum SessionState {
    Connecting,
    Connected,
    /// Waiting `next_in_secs` before attempt number `attempt`.
    Reconnecting {
        attempt: u32,
        next_in_secs: u64,
    },
    /// Connection lost; waiting for the user to press Reconnect.
    Lost {
        reason: String,
    },
    Closed {
        reason: String,
    },
}

/// Backoff for automatic reconnects.
#[derive(Debug, Clone)]
pub struct ReconnectConfig {
    pub base: Duration,
    pub max: Duration,
    /// Stop auto-retrying (and wait for the user) after this many failed attempts.
    pub max_attempts: Option<u32>,
}

impl Default for ReconnectConfig {
    fn default() -> Self {
        Self {
            base: Duration::from_secs(1),
            max: Duration::from_secs(60),
            max_attempts: None,
        }
    }
}

impl ReconnectConfig {
    /// Delay before attempt number `attempt` (1-based): base, 2x, 4x ... capped at max.
    pub fn delay(&self, attempt: u32) -> Duration {
        let factor = 1u32
            .checked_shl(attempt.saturating_sub(1).min(20))
            .unwrap_or(u32::MAX);
        self.base.saturating_mul(factor).min(self.max)
    }
}

/// Errors that retrying cannot fix: they need the user.
fn retryable(e: &SshError) -> bool {
    matches!(e.kind, ErrorKind::Connect(_) | ErrorKind::Session(_))
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
    Reconnect,
}

/// Per-session state shared between the manager API and the session's supervisor task.
#[derive(Default)]
struct Shared {
    /// `None` for telnet and while disconnected.
    ssh: Mutex<Option<Arc<SshSession>>>,
    sftp: tokio::sync::Mutex<Option<Arc<Sftp>>>,
    fwd: Mutex<Option<Arc<ForwardManager>>>,
    /// Forwards that should run whenever the session is connected (saved + ad-hoc), with stable ids.
    desired: Mutex<Vec<(u32, Forward)>>,
    next_forward: AtomicU32,
}

struct Active {
    tx: mpsc::UnboundedSender<Cmd>,
    shared: Arc<Shared>,
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
    connect_opts: Mutex<ConnectOptions>,
    reconnect_cfg: Mutex<ReconnectConfig>,
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
            connect_opts: Mutex::new(ConnectOptions::default()),
            reconnect_cfg: Mutex::new(ReconnectConfig::default()),
        })
    }

    pub fn set_connect_options(&self, o: ConnectOptions) {
        *lock(&self.connect_opts) = o;
    }

    pub fn set_reconnect_config(&self, c: ReconnectConfig) {
        *lock(&self.reconnect_cfg) = c;
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
                            Some(Cmd::Reconnect) => {}
                            Some(Cmd::Close) | None => { tap.finish(); break }
                        },
                    }
                }
            });
            lock(&self.sessions).insert(
                session_id.to_string(),
                Active {
                    tx,
                    shared: Arc::new(Shared::default()),
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
        self.events
            .session_state(session_id, SessionState::Connecting);
        let opts = lock(&self.connect_opts).clone();
        let session = SshSession::connect_with(&hops, self.known.clone(), self.policy(), &opts)
            .await
            .map_err(|e| e.to_string())?;
        let _ = lock(&self.known).save(&self.known_path);
        let session = Arc::new(session);
        let shell = if conn.no_shell {
            None
        } else {
            Some(
                session
                    .open_shell(cols, rows)
                    .await
                    .map_err(|e| e.to_string())?,
            )
        };
        let shared = Arc::new(Shared::default());
        {
            let mut d = lock(&shared.desired);
            for f in &conn.forwards {
                let id = shared.next_forward.fetch_add(1, Ordering::SeqCst) + 1;
                d.push((id, f.clone()));
            }
        }
        let mut sup = Supervisor {
            events,
            sid,
            tap,
            rx,
            shared: shared.clone(),
            hops,
            known: self.known.clone(),
            known_path: self.known_path.clone(),
            policy: self.policy(),
            opts,
            rc: lock(&self.reconnect_cfg).clone(),
            mode: conn.effective_reconnect(),
            no_shell: conn.no_shell,
            cols,
            rows,
            session: session.clone(),
            shell,
        };
        sup.install(session, false).await;
        lock(&self.sessions).insert(session_id.to_string(), Active { tx, shared });
        tokio::spawn(sup.run());
        Ok(())
    }

    /// Ask a lost session to reconnect now (also skips a pending auto-reconnect delay).
    pub fn reconnect(&self, session: &str) {
        self.send(session, Cmd::Reconnect);
    }

    fn shared(&self, session: &str) -> Result<Arc<Shared>, String> {
        lock(&self.sessions)
            .get(session)
            .map(|a| a.shared.clone())
            .ok_or_else(|| "no such session".to_string())
    }

    /// Add a forward to a live session (kept for reconnects). Failures show up as a `Failed` state.
    pub async fn forward_add(&self, session: &str, spec: Forward) -> Result<ForwardInfo, String> {
        spec.validate()?;
        let shared = self.shared(session)?;
        let id = shared.next_forward.fetch_add(1, Ordering::SeqCst) + 1;
        lock(&shared.desired).push((id, spec.clone()));
        let fm = lock(&shared.fwd).clone();
        let info = match fm {
            Some(fm) => fm.start_with_id(id, spec).await,
            None => ForwardInfo {
                id,
                spec,
                state: ForwardState::Failed {
                    reason: "not connected".into(),
                },
                connections: 0,
            },
        };
        self.events.forwards(session, forward_list(&shared));
        Ok(info)
    }

    pub async fn forward_stop(&self, session: &str, id: u32) -> Result<(), String> {
        let shared = self.shared(session)?;
        lock(&shared.desired).retain(|(i, _)| *i != id);
        let fm = lock(&shared.fwd).clone();
        if let Some(fm) = fm {
            fm.stop(id).await;
        }
        self.events.forwards(session, forward_list(&shared));
        Ok(())
    }

    pub fn forward_list(&self, session: &str) -> Result<Vec<ForwardInfo>, String> {
        let shared = self.shared(session)?;
        Ok(forward_list(&shared))
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
        let shared = self.shared(session)?;
        let ssh = lock(&shared.ssh)
            .clone()
            .ok_or("not connected (file transfer needs a live SSH session)")?;
        let mut guard = shared.sftp.lock().await;
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
        let this_sftp = lock(&self.sessions).get(&sess).map(|a| a.shared.clone());
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
            let Some(shared) = this_sftp else {
                return fail("no such session".into());
            };
            let Some(ssh) = lock(&shared.ssh).clone() else {
                return fail("not connected (file transfer needs a live SSH session)".into());
            };
            let sftp = {
                let mut g = shared.sftp.lock().await;
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

fn forward_list(shared: &Shared) -> Vec<ForwardInfo> {
    match lock(&shared.fwd).clone() {
        Some(fm) => fm.list(),
        None => lock(&shared.desired)
            .iter()
            .map(|(id, spec)| ForwardInfo {
                id: *id,
                spec: spec.clone(),
                state: ForwardState::Failed {
                    reason: "not connected".into(),
                },
                connections: 0,
            })
            .collect(),
    }
}

enum Ended {
    UserClose,
    Exited(u32),
    Dropped(String),
}

/// Owns one SSH session's lifetime: pumps shell I/O, detects a lost connection and
/// applies the reconnect policy (restoring forwards).
struct Supervisor<E: Events> {
    events: Arc<E>,
    sid: String,
    tap: Tap,
    rx: mpsc::UnboundedReceiver<Cmd>,
    shared: Arc<Shared>,
    hops: Vec<Hop>,
    known: Arc<Mutex<KnownHosts>>,
    known_path: PathBuf,
    policy: HostKeyPolicy,
    opts: ConnectOptions,
    rc: ReconnectConfig,
    mode: ReconnectPolicy,
    no_shell: bool,
    cols: u32,
    rows: u32,
    session: Arc<SshSession>,
    shell: Option<Shell>,
}

impl<E: Events> Supervisor<E> {
    /// Publish `session` (and its shell) as the live connection and (re)start forwards.
    async fn install(&mut self, session: Arc<SshSession>, announce: bool) {
        *lock(&self.shared.ssh) = Some(session.clone());
        *self.shared.sftp.lock().await = None;
        let fm = Arc::new(ForwardManager::new(session.clone()));
        let desired = lock(&self.shared.desired).clone();
        for (id, spec) in desired {
            fm.start_with_id(id, spec).await;
        }
        *lock(&self.shared.fwd) = Some(fm);
        self.session = session;
        if announce {
            self.note("\r\n\x1b[32m[reconnected]\x1b[0m\r\n");
        }
        self.events
            .session_state(&self.sid, SessionState::Connected);
        self.events.forwards(&self.sid, forward_list(&self.shared));
    }

    /// Terminal-visible status line (also recorded by extensions).
    fn note(&mut self, text: &str) {
        self.tap.data(text.as_bytes());
        self.events.data(&self.sid, text.as_bytes().to_vec());
    }

    fn teardown(&mut self) {
        *lock(&self.shared.ssh) = None;
        *lock(&self.shared.fwd) = None;
        self.shell = None;
        self.events.forwards(&self.sid, forward_list(&self.shared));
    }

    async fn connected(&mut self) -> Ended {
        loop {
            let Self {
                shell,
                rx,
                session,
                tap,
                events,
                sid,
                cols,
                rows,
                ..
            } = self;
            tokio::select! {
                ev = async {
                    match shell.as_mut() {
                        Some(s) => s.next().await,
                        None => std::future::pending().await,
                    }
                } => match ev {
                    ShellEvent::Data(d) => { tap.data(&d); events.data(sid, d); }
                    ShellEvent::Exit(code) => return Ended::Exited(code),
                    ShellEvent::Closed => return Ended::Dropped("connection closed".into()),
                },
                cmd = rx.recv() => match cmd {
                    Some(Cmd::Write(b)) => { if let Some(s) = shell.as_ref() { let _ = s.write(&b).await; } }
                    Some(Cmd::Resize(c, r)) => {
                        *cols = c;
                        *rows = r;
                        if let Some(s) = shell.as_ref() { let _ = s.resize(c, r).await; }
                    }
                    Some(Cmd::Reconnect) => {}
                    Some(Cmd::Close) | None => return Ended::UserClose,
                },
                () = session.closed() => return Ended::Dropped("connection lost".into()),
            }
        }
    }

    async fn try_connect(&mut self) -> Result<(), SshError> {
        let s = SshSession::connect_with(
            &self.hops,
            self.known.clone(),
            self.policy.clone(),
            &self.opts,
        )
        .await?;
        let _ = lock(&self.known).save(&self.known_path);
        let s = Arc::new(s);
        let shell = if self.no_shell {
            None
        } else {
            let h = |e: russh::Error| SshError {
                index: self.hops.len().saturating_sub(1),
                name: String::new(),
                kind: ErrorKind::Session(e.to_string()),
            };
            Some(s.open_shell(self.cols, self.rows).await.map_err(h)?)
        };
        self.shell = shell;
        self.install(s, true).await;
        Ok(())
    }

    /// Wait until the user presses Reconnect (true) or closes the session (false).
    async fn wait_for_user(&mut self, mut reason: String) -> bool {
        loop {
            self.events.session_state(
                &self.sid,
                SessionState::Lost {
                    reason: reason.clone(),
                },
            );
            loop {
                match self.rx.recv().await {
                    Some(Cmd::Reconnect) => break,
                    Some(Cmd::Resize(c, r)) => {
                        self.cols = c;
                        self.rows = r;
                    }
                    Some(Cmd::Close) | None => return false,
                    Some(Cmd::Write(_)) => {}
                }
            }
            self.events.session_state(
                &self.sid,
                SessionState::Reconnecting {
                    attempt: 1,
                    next_in_secs: 0,
                },
            );
            match self.try_connect().await {
                Ok(()) => return true,
                Err(e) => reason = e.to_string(),
            }
        }
    }

    /// Retry with backoff until connected (true), the user closes (false), or retrying
    /// cannot help (falls back to waiting for the user).
    async fn auto_reconnect(&mut self, reason: String) -> bool {
        let mut attempt = 0u32;
        loop {
            attempt += 1;
            if self.rc.max_attempts.is_some_and(|m| attempt > m) {
                return self
                    .wait_for_user(format!("{reason} (gave up after {} attempts)", attempt - 1))
                    .await;
            }
            let delay = self.rc.delay(attempt);
            self.events.session_state(
                &self.sid,
                SessionState::Reconnecting {
                    attempt,
                    next_in_secs: delay.as_secs(),
                },
            );
            let sleep = tokio::time::sleep(delay);
            tokio::pin!(sleep);
            loop {
                tokio::select! {
                    () = &mut sleep => break,
                    cmd = self.rx.recv() => match cmd {
                        Some(Cmd::Reconnect) => break,
                        Some(Cmd::Resize(c, r)) => { self.cols = c; self.rows = r; }
                        Some(Cmd::Close) | None => return false,
                        Some(Cmd::Write(_)) => {}
                    },
                }
            }
            match self.try_connect().await {
                Ok(()) => return true,
                Err(e) if !retryable(&e) => return self.wait_for_user(e.to_string()).await,
                Err(_) => {}
            }
        }
    }

    async fn run(mut self) {
        loop {
            match self.connected().await {
                Ended::UserClose => {
                    self.teardown();
                    self.tap.finish();
                    return;
                }
                Ended::Exited(code) => {
                    self.teardown();
                    self.tap.finish();
                    let reason = format!("exit status {code}");
                    self.events.session_state(
                        &self.sid,
                        SessionState::Closed {
                            reason: reason.clone(),
                        },
                    );
                    self.events.closed(&self.sid, reason);
                    return;
                }
                Ended::Dropped(reason) => {
                    self.teardown();
                    self.note("\r\n\x1b[33m[connection lost]\x1b[0m\r\n");
                    let again = match self.mode {
                        ReconnectPolicy::Off => false,
                        ReconnectPolicy::Ask => self.wait_for_user(reason.clone()).await,
                        ReconnectPolicy::Auto => self.auto_reconnect(reason.clone()).await,
                    };
                    if !again {
                        self.tap.finish();
                        if self.mode == ReconnectPolicy::Off {
                            self.events.session_state(
                                &self.sid,
                                SessionState::Closed {
                                    reason: reason.clone(),
                                },
                            );
                            self.events.closed(&self.sid, reason);
                        }
                        return;
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_doubles_and_caps() {
        let c = ReconnectConfig {
            base: Duration::from_secs(1),
            max: Duration::from_secs(60),
            max_attempts: None,
        };
        let secs: Vec<u64> = (1..=9).map(|a| c.delay(a).as_secs()).collect();
        assert_eq!(secs, [1, 2, 4, 8, 16, 32, 60, 60, 60]);
        assert_eq!(c.delay(0).as_secs(), 1, "attempt 0 behaves like 1");
        assert_eq!(c.delay(u32::MAX).as_secs(), 60, "no overflow");
    }

    #[test]
    fn only_transport_errors_are_retried() {
        let e = |kind| SshError {
            index: 0,
            name: "h".into(),
            kind,
        };
        assert!(retryable(&e(ErrorKind::Connect("refused".into()))));
        assert!(retryable(&e(ErrorKind::Session("reset".into()))));
        assert!(!retryable(&e(ErrorKind::HostKeyChanged)));
        assert!(!retryable(&e(ErrorKind::HostKeyRejected)));
        assert!(!retryable(&e(ErrorKind::AuthFailed)));
        assert!(!retryable(&e(ErrorKind::BadKey("x".into()))));
    }

    #[test]
    fn session_state_json_shape() {
        let j = serde_json::to_string(&SessionState::Reconnecting {
            attempt: 2,
            next_in_secs: 4,
        })
        .unwrap();
        assert_eq!(j, r#"{"state":"reconnecting","attempt":2,"nextInSecs":4}"#);
        assert_eq!(
            serde_json::to_string(&SessionState::Connected).unwrap(),
            r#"{"state":"connected"}"#
        );
    }
}
