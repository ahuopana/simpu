//! Plain TCP helpers for tunnel and keepalive tests.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

/// Echoes every byte back. Use as a forward destination.
pub struct EchoServer {
    pub port: u16,
}

impl EchoServer {
    pub async fn start() -> Self {
        let l = TcpListener::bind(("127.0.0.1", 0)).await.expect("bind");
        let port = l.local_addr().expect("addr").port();
        tokio::spawn(async move {
            while let Ok((mut s, _)) = l.accept().await {
                tokio::spawn(async move {
                    let mut buf = [0u8; 4096];
                    while let Ok(n) = s.read(&mut buf).await {
                        if n == 0 || s.write_all(&buf[..n]).await.is_err() {
                            break;
                        }
                    }
                });
            }
        });
        Self { port }
    }
}

/// TCP proxy to `target` that can be switched to silently swallow all traffic
/// in both directions without closing sockets (a dead link).
pub struct Blackhole {
    pub port: u16,
    paused: Arc<AtomicBool>,
}

impl Blackhole {
    pub async fn start(target_port: u16) -> Self {
        let l = TcpListener::bind(("127.0.0.1", 0)).await.expect("bind");
        let port = l.local_addr().expect("addr").port();
        let paused = Arc::new(AtomicBool::new(false));
        let p = paused.clone();
        tokio::spawn(async move {
            while let Ok((client, _)) = l.accept().await {
                let Ok(server) = TcpStream::connect(("127.0.0.1", target_port)).await else {
                    continue;
                };
                let (mut cr, mut cw) = client.into_split();
                let (mut sr, mut sw) = server.into_split();
                let (p1, p2) = (p.clone(), p.clone());
                tokio::spawn(async move { pump(&mut cr, &mut sw, p1).await });
                tokio::spawn(async move { pump(&mut sr, &mut cw, p2).await });
            }
        });
        Self { port, paused }
    }

    /// Swallow traffic from now on.
    pub fn pause(&self) {
        self.paused.store(true, Ordering::SeqCst);
    }

    pub fn resume(&self) {
        self.paused.store(false, Ordering::SeqCst);
    }
}

async fn pump<R, W>(r: &mut R, w: &mut W, paused: Arc<AtomicBool>)
where
    R: AsyncReadExt + Unpin,
    W: AsyncWriteExt + Unpin,
{
    let mut buf = [0u8; 8192];
    while let Ok(n) = r.read(&mut buf).await {
        if n == 0 {
            break;
        }
        if paused.load(Ordering::SeqCst) {
            continue; // dropped on the floor
        }
        if w.write_all(&buf[..n]).await.is_err() {
            break;
        }
    }
}

/// A tiny telnet server: sends `login: `, answers every received chunk with `got:<chunk>`,
/// and can drop all clients, stop listening and start again on the same port.
pub struct TelnetServer {
    pub port: u16,
    pub accepted: Arc<std::sync::atomic::AtomicUsize>,
    kill: tokio::sync::watch::Sender<u64>,
    task: std::sync::Mutex<Option<tokio::task::JoinHandle<()>>>,
}

impl TelnetServer {
    fn spawn(
        l: TcpListener,
        count: Arc<std::sync::atomic::AtomicUsize>,
        rx: tokio::sync::watch::Receiver<u64>,
    ) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            while let Ok((mut s, _)) = l.accept().await {
                count.fetch_add(1, Ordering::SeqCst);
                let mut rx = rx.clone();
                let generation = *rx.borrow();
                tokio::spawn(async move {
                    let _ = s.write_all(b"login: ").await;
                    let mut buf = [0u8; 1024];
                    loop {
                        tokio::select! {
                            n = s.read(&mut buf) => match n {
                                Ok(0) | Err(_) => break,
                                Ok(n) => {
                                    let mut out = b"got:".to_vec();
                                    out.extend_from_slice(&buf[..n]);
                                    if s.write_all(&out).await.is_err() { break; }
                                }
                            },
                            changed = rx.changed() => {
                                if changed.is_err() || *rx.borrow() != generation { break; }
                            }
                        }
                    }
                });
            }
        })
    }

    pub async fn start() -> Self {
        let l = TcpListener::bind(("127.0.0.1", 0)).await.expect("bind");
        let port = l.local_addr().expect("addr").port();
        let accepted = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let (kill, rx) = tokio::sync::watch::channel(0u64);
        let task = Self::spawn(l, accepted.clone(), rx);
        Self {
            port,
            accepted,
            kill,
            task: std::sync::Mutex::new(Some(task)),
        }
    }

    /// Close every client connection (the server keeps accepting new ones).
    pub fn kill_connections(&self) {
        self.kill.send_modify(|g| *g += 1);
    }

    /// Drop all clients and stop listening: new connections are refused until [`restart`](Self::restart).
    pub fn stop(&self) {
        self.kill_connections();
        if let Some(t) = self.task.lock().unwrap_or_else(|p| p.into_inner()).take() {
            t.abort();
        }
    }

    /// Listen again on the same port.
    pub async fn restart(&self) {
        let l = TcpListener::bind(("127.0.0.1", self.port))
            .await
            .expect("rebind");
        let task = Self::spawn(l, self.accepted.clone(), self.kill.subscribe());
        *self.task.lock().unwrap_or_else(|p| p.into_inner()) = Some(task);
    }
}
