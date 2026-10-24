//! SSH client sessions, including multi-hop jump host chains.

use crate::credentials::{CredentialError, Credentials, Secret};
use crate::hostkeys::{KnownHosts, Verdict};
use crate::jump::{resolve_chain, JumpError};
use crate::model::{Connection, SecretKind, Tree};
use russh::client::{self, Handle};
use russh::keys::PublicKeyOrCertificate;
use russh::{Channel, ChannelMsg};
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use thiserror::Error;
use tokio::sync::watch;

/// How to authenticate to one hop.
#[derive(Clone)]
pub enum HopAuth {
    /// "none" authentication (servers that allow it).
    None,
    Password(Secret),
    /// Private key in OpenSSH/PEM format, with an optional passphrase.
    Key {
        pem: Secret,
        passphrase: Option<Secret>,
    },
}

/// One hop of a connection: a jump host or the final target.
#[derive(Clone)]
pub struct Hop {
    pub name: String,
    pub host: String,
    pub port: u16,
    pub username: String,
    pub auth: HopAuth,
}

#[derive(Debug, Error)]
pub enum PlanError {
    #[error(transparent)]
    Jump(#[from] JumpError),
    #[error("credentials for {hop}: {source}")]
    Credential {
        hop: String,
        provider: String,
        source: CredentialError,
    },
}

/// Expand jump hosts and fetch each hop's password from the credential providers.
pub fn plan_hops(
    tree: &Tree,
    target: &Connection,
    creds: &Credentials,
) -> Result<Vec<Hop>, PlanError> {
    resolve_chain(tree, target)?
        .into_iter()
        .map(|c| {
            let auth = match &c.secret {
                Some(r) => {
                    let secret = creds.resolve(r).map_err(|source| PlanError::Credential {
                        hop: c.name.clone(),
                        provider: r.provider.clone(),
                        source,
                    })?;
                    match r.kind {
                        SecretKind::Password => HopAuth::Password(secret),
                        SecretKind::KeyFile => HopAuth::Key {
                            pem: secret,
                            passphrase: None,
                        },
                    }
                }
                None => HopAuth::None,
            };
            Ok(Hop {
                name: c.name.clone(),
                host: c.host.clone(),
                port: c.effective_port(),
                username: c.username.clone(),
                auth,
            })
        })
        .collect()
}

#[derive(Debug, Error)]
pub enum ErrorKind {
    #[error("connection failed: {0}")]
    Connect(String),
    #[error("host key rejected by user")]
    HostKeyRejected,
    #[error("HOST KEY CHANGED (possible man-in-the-middle attack)")]
    HostKeyChanged,
    #[error("authentication failed")]
    AuthFailed,
    #[error("cannot use private key: {0}")]
    BadKey(String),
    #[error("session error: {0}")]
    Session(String),
}

/// An error that names the hop where it happened.
#[derive(Debug, Error)]
#[error("hop {index} ({name}): {kind}")]
pub struct SshError {
    pub index: usize,
    pub name: String,
    pub kind: ErrorKind,
}

/// What the user is asked when meeting an unknown host.
pub struct HostKeyPrompt {
    pub host: String,
    pub port: u16,
    pub key: String,
}

pub type PromptFuture = Pin<Box<dyn Future<Output = bool> + Send>>;
/// Called for unknown hosts; return true to trust and remember the key.
pub type HostKeyPolicy = Arc<dyn Fn(HostKeyPrompt) -> PromptFuture + Send + Sync>;

pub fn always_trust() -> HostKeyPolicy {
    Arc::new(|_| Box::pin(async { true }))
}

pub fn never_trust() -> HostKeyPolicy {
    Arc::new(|_| Box::pin(async { false }))
}

/// Remote (`-R`) forwards of a session: `(bind host, bind port)` -> local destination.
pub type RemoteForwards = Arc<Mutex<HashMap<(String, u32), (String, u16)>>>;

/// Transport tuning.
#[derive(Debug, Clone)]
pub struct ConnectOptions {
    /// Send an SSH keepalive this often (None disables).
    pub keepalive_interval: Option<Duration>,
    /// Give up after this many unanswered keepalives.
    pub keepalive_max: usize,
    /// Abort establishing a connection (all hops, including authentication) after this long.
    pub connect_timeout: Duration,
}

impl Default for ConnectOptions {
    fn default() -> Self {
        Self {
            keepalive_interval: Some(Duration::from_secs(30)),
            keepalive_max: 3,
            connect_timeout: Duration::from_secs(20),
        }
    }
}

struct ClientHandler {
    remote: RemoteForwards,
    closed: Arc<watch::Sender<bool>>,
    host: String,
    port: u16,
    known: Arc<Mutex<KnownHosts>>,
    policy: HostKeyPolicy,
    failure: Arc<Mutex<Option<ErrorKind>>>,
}

impl client::Handler for ClientHandler {
    type Error = russh::Error;

    async fn disconnected(
        &mut self,
        _reason: russh::client::DisconnectReason<Self::Error>,
    ) -> Result<(), Self::Error> {
        // Whatever the reason, the transport is gone; supervisors watch this.
        self.closed.send_replace(true);
        Ok(())
    }

    async fn server_channel_open_forwarded_tcpip(
        &mut self,
        channel: russh::Channel<client::Msg>,
        connected_address: &str,
        connected_port: u32,
        _originator_address: &str,
        _originator_port: u32,
        reply: client::ChannelOpenHandle,
        _session: &mut client::Session,
    ) -> Result<(), Self::Error> {
        let dest = lock(&self.remote)
            .get(&(connected_address.to_string(), connected_port))
            .cloned();
        let Some((host, port)) = dest else {
            return Ok(());
        }; // dropping `reply` rejects
        let Ok(mut local) = tokio::net::TcpStream::connect((host.as_str(), port)).await else {
            return Ok(());
        };
        reply.accept().await;
        tokio::spawn(async move {
            let mut remote = channel.into_stream();
            let _ = tokio::io::copy_bidirectional(&mut remote, &mut local).await;
        });
        Ok(())
    }

    async fn check_server_key(
        &mut self,
        key: &PublicKeyOrCertificate,
    ) -> Result<bool, Self::Error> {
        let PublicKeyOrCertificate::PublicKey { key, .. } = key else {
            return Ok(false);
        };
        let line = key.to_openssh().unwrap_or_default();
        let verdict = lock(&self.known).verify(&self.host, self.port, &line);
        let fail = |k| *lock(&self.failure) = Some(k);
        match verdict {
            Verdict::Trusted => Ok(true),
            Verdict::Changed { .. } => {
                fail(ErrorKind::HostKeyChanged);
                Ok(false)
            }
            Verdict::Unknown => {
                let prompt = HostKeyPrompt {
                    host: self.host.clone(),
                    port: self.port,
                    key: line.clone(),
                };
                if (self.policy)(prompt).await {
                    lock(&self.known).add(&self.host, self.port, &line);
                    Ok(true)
                } else {
                    fail(ErrorKind::HostKeyRejected);
                    Ok(false)
                }
            }
        }
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

/// Output of a remote command.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct ExecOutput {
    pub stdout: Vec<u8>,
    pub exit_status: Option<u32>,
}

/// A connected, authenticated session on the final hop. Keeps the jump
/// sessions alive for as long as it exists.
pub struct SshSession {
    handle: Handle<ClientHandler>,
    _jumps: Vec<Handle<ClientHandler>>,
    remote: RemoteForwards,
    closed: watch::Receiver<bool>,
}

impl SshSession {
    /// Connect through all `hops` in order; the last one is the target.
    pub async fn connect(
        hops: &[Hop],
        known: Arc<Mutex<KnownHosts>>,
        policy: HostKeyPolicy,
    ) -> Result<Self, SshError> {
        Self::connect_with(hops, known, policy, &ConnectOptions::default()).await
    }

    /// Like [`connect`](Self::connect) with explicit transport options.
    pub async fn connect_with(
        hops: &[Hop],
        known: Arc<Mutex<KnownHosts>>,
        policy: HostKeyPolicy,
        opts: &ConnectOptions,
    ) -> Result<Self, SshError> {
        // The timeout does not run while the user is being asked about a host key.
        let prompting = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counted: HostKeyPolicy = {
            let (inner, prompting) = (policy, prompting.clone());
            Arc::new(move |p| {
                let (inner, prompting) = (inner.clone(), prompting.clone());
                Box::pin(async move {
                    prompting.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    let answer = inner(p).await;
                    prompting.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
                    answer
                })
            })
        };
        let fut = Self::connect_inner(hops, known, counted, opts);
        tokio::pin!(fut);
        loop {
            tokio::select! {
                r = &mut fut => return r,
                () = tokio::time::sleep(opts.connect_timeout) => {
                    if prompting.load(std::sync::atomic::Ordering::SeqCst) == 0 {
                        return Err(SshError {
                            index: hops.len().saturating_sub(1),
                            name: hops.last().map(|h| h.name.clone()).unwrap_or_default(),
                            kind: ErrorKind::Connect(format!("timed out after {}s", opts.connect_timeout.as_secs_f32())),
                        });
                    }
                }
            }
        }
    }

    async fn connect_inner(
        hops: &[Hop],
        known: Arc<Mutex<KnownHosts>>,
        policy: HostKeyPolicy,
        opts: &ConnectOptions,
    ) -> Result<Self, SshError> {
        let config = Arc::new(client::Config {
            keepalive_interval: opts.keepalive_interval,
            keepalive_max: opts.keepalive_max,
            ..Default::default()
        });
        let remote: RemoteForwards = Arc::default();
        let (closed_tx, closed_rx) = watch::channel(false);
        let closed_tx = Arc::new(closed_tx);
        let mut sessions: Vec<Handle<ClientHandler>> = Vec::new();
        for (index, hop) in hops.iter().enumerate() {
            let err = |kind| SshError {
                index,
                name: hop.name.clone(),
                kind,
            };
            let failure = Arc::new(Mutex::new(None));
            let handler = ClientHandler {
                remote: remote.clone(),
                closed: closed_tx.clone(),
                host: hop.host.clone(),
                port: hop.port,
                known: known.clone(),
                policy: policy.clone(),
                failure: failure.clone(),
            };
            let connected = match sessions.last() {
                None => {
                    client::connect(config.clone(), (hop.host.as_str(), hop.port), handler).await
                }
                Some(prev) => {
                    let ch = prev
                        .channel_open_direct_tcpip(
                            hop.host.clone(),
                            hop.port.into(),
                            "127.0.0.1",
                            0,
                        )
                        .await
                        .map_err(|e| err(ErrorKind::Connect(e.to_string())))?;
                    client::connect_stream(config.clone(), ch.into_stream(), handler).await
                }
            };
            let mut handle = match connected {
                Ok(h) => h,
                Err(e) => {
                    let kind = lock(&failure)
                        .take()
                        .unwrap_or(ErrorKind::Connect(e.to_string()));
                    return Err(err(kind));
                }
            };
            let session_err = |e: russh::Error| err(ErrorKind::Session(e.to_string()));
            let ok = match &hop.auth {
                HopAuth::Password(p) => handle
                    .authenticate_password(hop.username.clone(), p.expose())
                    .await
                    .map_err(session_err)?
                    .success(),
                HopAuth::None => handle
                    .authenticate_none(hop.username.clone())
                    .await
                    .map_err(session_err)?
                    .success(),
                HopAuth::Key { pem, passphrase } => {
                    let key = russh::keys::decode_secret_key(
                        pem.expose(),
                        passphrase.as_ref().map(|p| p.expose()),
                    )
                    .map_err(|e| err(ErrorKind::BadKey(e.to_string())))?;
                    let hash = handle
                        .best_supported_rsa_hash()
                        .await
                        .map_err(session_err)?
                        .flatten();
                    handle
                        .authenticate_publickey(
                            hop.username.clone(),
                            russh::keys::PrivateKeyWithHashAlg::new(Arc::new(key), hash),
                        )
                        .await
                        .map_err(session_err)?
                        .success()
                }
            };
            if !ok {
                return Err(err(ErrorKind::AuthFailed));
            }
            sessions.push(handle);
        }
        let handle = sessions.pop().ok_or(SshError {
            index: 0,
            name: String::new(),
            kind: ErrorKind::Connect("no hops".into()),
        })?;
        Ok(Self {
            handle,
            _jumps: sessions,
            remote,
            closed: closed_rx,
        })
    }

    /// True once the transport (of the target or any jump host) is gone.
    pub fn is_closed(&self) -> bool {
        *self.closed.borrow() || self.handle.is_closed()
    }

    /// Resolves when the connection is lost (keepalive timeout, server disconnect, I/O error).
    pub async fn closed(&self) {
        let mut rx = self.closed.clone();
        while !*rx.borrow() {
            if rx.changed().await.is_err() {
                break;
            }
        }
    }

    /// Open a `direct-tcpip` channel (local and dynamic forwards, jump hosts).
    pub async fn open_direct_tcpip(
        &self,
        host: &str,
        port: u16,
        origin: &str,
    ) -> Result<russh::ChannelStream<client::Msg>, russh::Error> {
        let ch = self
            .handle
            .channel_open_direct_tcpip(host, u32::from(port), origin, 0)
            .await?;
        Ok(ch.into_stream())
    }

    /// Ask the server to listen on `bind_host:bind_port` and send connections to
    /// `dest_host:dest_port` from this machine. Returns the port actually bound.
    pub async fn request_remote_forward(
        &self,
        bind_host: &str,
        bind_port: u16,
        dest_host: &str,
        dest_port: u16,
    ) -> Result<u16, russh::Error> {
        // Register first: connections may arrive right after the server confirms.
        let key_port = u32::from(bind_port);
        lock(&self.remote).insert(
            (bind_host.to_string(), key_port),
            (dest_host.to_string(), dest_port),
        );
        match self.handle.tcpip_forward(bind_host, key_port).await {
            Ok(actual) => {
                let actual = if actual == 0 { key_port } else { actual };
                if actual != key_port {
                    let mut r = lock(&self.remote);
                    if let Some(v) = r.remove(&(bind_host.to_string(), key_port)) {
                        r.insert((bind_host.to_string(), actual), v);
                    }
                }
                Ok(actual as u16)
            }
            Err(e) => {
                lock(&self.remote).remove(&(bind_host.to_string(), key_port));
                Err(e)
            }
        }
    }

    pub async fn cancel_remote_forward(
        &self,
        bind_host: &str,
        bind_port: u16,
    ) -> Result<(), russh::Error> {
        lock(&self.remote).remove(&(bind_host.to_string(), u32::from(bind_port)));
        self.handle
            .cancel_tcpip_forward(bind_host, u32::from(bind_port))
            .await
    }

    /// Run a command and collect stdout.
    pub async fn exec(&self, command: &str) -> Result<ExecOutput, russh::Error> {
        let mut ch = self.handle.channel_open_session().await?;
        ch.exec(true, command).await?;
        let mut out = ExecOutput::default();
        while let Some(msg) = ch.wait().await {
            match msg {
                ChannelMsg::Data { data } => out.stdout.extend_from_slice(&data),
                ChannelMsg::ExitStatus { exit_status } => out.exit_status = Some(exit_status),
                _ => {}
            }
        }
        Ok(out)
    }
}

/// Something that happened on an interactive shell.
#[derive(Debug, PartialEq, Eq)]
pub enum ShellEvent {
    Data(Vec<u8>),
    Exit(u32),
    Closed,
}

/// An interactive shell with a PTY; the terminal emulator sits on top of this.
pub struct Shell {
    channel: Channel<client::Msg>,
}

impl Shell {
    pub async fn write(&self, bytes: &[u8]) -> Result<(), russh::Error> {
        self.channel.data_bytes(bytes.to_vec()).await
    }

    pub async fn resize(&self, cols: u32, rows: u32) -> Result<(), russh::Error> {
        self.channel.window_change(cols, rows, 0, 0).await
    }

    /// Next event; `Closed` once the channel is gone. stderr is merged into `Data`.
    pub async fn next(&mut self) -> ShellEvent {
        loop {
            match self.channel.wait().await {
                Some(ChannelMsg::Data { data }) => return ShellEvent::Data(data.to_vec()),
                Some(ChannelMsg::ExtendedData { data, .. }) => {
                    return ShellEvent::Data(data.to_vec())
                }
                Some(ChannelMsg::ExitStatus { exit_status }) => {
                    return ShellEvent::Exit(exit_status)
                }
                Some(_) => continue,
                None => return ShellEvent::Closed,
            }
        }
    }
}

impl SshSession {
    pub(crate) async fn open_channel(&self) -> Result<Channel<client::Msg>, russh::Error> {
        self.handle.channel_open_session().await
    }

    /// Open an interactive shell with a pseudo terminal of the given size.
    pub async fn open_shell(&self, cols: u32, rows: u32) -> Result<Shell, russh::Error> {
        let channel = self.handle.channel_open_session().await?;
        channel
            .request_pty(true, "xterm-256color", cols, rows, 0, 0, &[])
            .await?;
        channel.request_shell(true).await?;
        Ok(Shell { channel })
    }
}
