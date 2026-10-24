//! Port forwarding for one SSH session: local (`-L`), remote (`-R`), dynamic (`-D`, SOCKS5).

use crate::model::{Forward, ForwardKind};
use crate::socks5;
use crate::ssh::SshSession;
use serde::Serialize;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "lowercase")]
pub enum ForwardState {
    /// Listening; `port` is the port actually bound (differs from the spec for port 0).
    Listening {
        port: u16,
    },
    Failed {
        reason: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ForwardInfo {
    pub id: u32,
    pub spec: Forward,
    #[serde(flatten)]
    pub state: ForwardState,
    /// Connections accepted so far.
    pub connections: u64,
}

struct Item {
    spec: Forward,
    state: ForwardState,
    connections: Arc<AtomicU64>,
    task: Option<JoinHandle<()>>,
}

#[derive(Default)]
struct Inner {
    items: HashMap<u32, Item>,
}

pub struct ForwardManager {
    session: Arc<SshSession>,
    inner: Mutex<Inner>,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

impl ForwardManager {
    pub fn new(session: Arc<SshSession>) -> Self {
        Self {
            session,
            inner: Mutex::new(Inner::default()),
        }
    }

    /// Start a forward. Failures (port in use, server refuses) are recorded in the
    /// returned info as `Failed`; they never affect the session.
    pub async fn start(&self, spec: Forward) -> ForwardInfo {
        let id = lock(&self.inner).items.keys().max().map_or(1, |m| m + 1);
        self.start_with_id(id, spec).await
    }

    /// Like [`start`](Self::start) with a caller-chosen id (stable across reconnects).
    pub async fn start_with_id(&self, id: u32, spec: Forward) -> ForwardInfo {
        let connections = Arc::new(AtomicU64::new(0));
        let (state, task) = match self.launch(&spec, connections.clone()).await {
            Ok((port, task)) => (ForwardState::Listening { port }, task),
            Err(reason) => (ForwardState::Failed { reason }, None),
        };
        let mut g = lock(&self.inner);
        let info = ForwardInfo {
            id,
            spec: spec.clone(),
            state: state.clone(),
            connections: 0,
        };
        g.items.insert(
            id,
            Item {
                spec,
                state,
                connections,
                task,
            },
        );
        info
    }

    async fn launch(
        &self,
        spec: &Forward,
        count: Arc<AtomicU64>,
    ) -> Result<(u16, Option<JoinHandle<()>>), String> {
        match spec.kind {
            ForwardKind::Remote => {
                let (host, port) = dest(spec)?;
                let bound = self
                    .session
                    .request_remote_forward(&spec.bind_host, spec.bind_port, &host, port)
                    .await
                    .map_err(|e| format!("server refused remote forward: {e}"))?;
                Ok((bound, None))
            }
            ForwardKind::Local | ForwardKind::Dynamic => {
                let dest = if spec.kind == ForwardKind::Local {
                    Some(dest(spec)?)
                } else {
                    None
                };
                let listener = TcpListener::bind((spec.bind_host.as_str(), spec.bind_port))
                    .await
                    .map_err(|e| {
                        format!(
                            "cannot listen on {}:{}: {e}",
                            spec.bind_host, spec.bind_port
                        )
                    })?;
                let port = listener.local_addr().map_err(|e| e.to_string())?.port();
                let session = self.session.clone();
                let task = tokio::spawn(async move {
                    while let Ok((sock, peer)) = listener.accept().await {
                        count.fetch_add(1, Ordering::Relaxed);
                        let session = session.clone();
                        let dest = dest.clone();
                        tokio::spawn(async move {
                            let _ = serve_connection(session, sock, peer.to_string(), dest).await;
                        });
                    }
                });
                Ok((port, Some(task)))
            }
        }
    }

    pub async fn stop(&self, id: u32) -> bool {
        let item = lock(&self.inner).items.remove(&id);
        let Some(item) = item else { return false };
        if let Some(t) = item.task {
            t.abort();
        }
        if item.spec.kind == ForwardKind::Remote {
            if let ForwardState::Listening { port } = item.state {
                let _ = self
                    .session
                    .cancel_remote_forward(&item.spec.bind_host, port)
                    .await;
            }
        }
        true
    }

    pub fn list(&self) -> Vec<ForwardInfo> {
        let g = lock(&self.inner);
        let mut v: Vec<ForwardInfo> = g
            .items
            .iter()
            .map(|(id, i)| ForwardInfo {
                id: *id,
                spec: i.spec.clone(),
                state: i.state.clone(),
                connections: i.connections.load(Ordering::Relaxed),
            })
            .collect();
        v.sort_by_key(|i| i.id);
        v
    }
}

impl Drop for ForwardManager {
    fn drop(&mut self) {
        for item in lock(&self.inner).items.values() {
            if let Some(t) = &item.task {
                t.abort();
            }
        }
    }
}

fn dest(spec: &Forward) -> Result<(String, u16), String> {
    match (&spec.dest_host, spec.dest_port) {
        (Some(h), Some(p)) if !h.is_empty() && p != 0 => Ok((h.clone(), p)),
        _ => Err("destination host and port are required".into()),
    }
}

async fn serve_connection(
    session: Arc<SshSession>,
    mut sock: TcpStream,
    origin: String,
    fixed: Option<(String, u16)>,
) -> Result<(), String> {
    let socks = fixed.is_none();
    let (host, port) = match fixed {
        Some(d) => d,
        None => socks5::handshake(&mut sock)
            .await
            .map_err(|e| e.to_string())?,
    };
    let mut channel = match session.open_direct_tcpip(&host, port, &origin).await {
        Ok(c) => c,
        Err(e) => {
            if socks {
                let _ = socks5::reply(&mut sock, socks5::REP_GENERAL_FAILURE).await;
            }
            return Err(e.to_string());
        }
    };
    if socks {
        socks5::reply(&mut sock, socks5::REP_SUCCEEDED)
            .await
            .map_err(|e| e.to_string())?;
    }
    tokio::io::copy_bidirectional(&mut sock, &mut channel)
        .await
        .map(|_| ())
        .map_err(|e| e.to_string())
}
