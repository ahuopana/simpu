//! SSH client sessions, including multi-hop jump host chains.

use crate::credentials::{CredentialError, Credentials, Secret};
use crate::hostkeys::{KnownHosts, Verdict};
use crate::jump::{resolve_chain, JumpError};
use crate::model::{Connection, SecretKind, Tree};
use russh::client::{self, Handle};
use russh::keys::PublicKeyOrCertificate;
use russh::{Channel, ChannelMsg};
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use thiserror::Error;

/// How to authenticate to one hop.
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

struct ClientHandler {
    host: String,
    port: u16,
    known: Arc<Mutex<KnownHosts>>,
    policy: HostKeyPolicy,
    failure: Arc<Mutex<Option<ErrorKind>>>,
}

impl client::Handler for ClientHandler {
    type Error = russh::Error;

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
}

impl SshSession {
    /// Connect through all `hops` in order; the last one is the target.
    pub async fn connect(
        hops: &[Hop],
        known: Arc<Mutex<KnownHosts>>,
        policy: HostKeyPolicy,
    ) -> Result<Self, SshError> {
        let config = Arc::new(client::Config::default());
        let mut sessions: Vec<Handle<ClientHandler>> = Vec::new();
        for (index, hop) in hops.iter().enumerate() {
            let err = |kind| SshError {
                index,
                name: hop.name.clone(),
                kind,
            };
            let failure = Arc::new(Mutex::new(None));
            let handler = ClientHandler {
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
        })
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
