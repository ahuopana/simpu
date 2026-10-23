//! In-process SSH server for tests: password auth, `exec`, and direct-tcpip
//! forwarding (so jump host chains can be tested without Docker).

use russh::server::{self, Auth, Msg, Server as _, Session};
use russh::{Channel, ChannelId};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use tokio::net::{TcpListener, TcpStream};

pub struct TestServer {
    pub port: u16,
    pub forwards: Arc<AtomicUsize>,
    pub host_key_line: String,
}

#[derive(Clone)]
struct Handler {
    label: String,
    user: String,
    password: String,
    forwards: Arc<AtomicUsize>,
    shells: Vec<ChannelId>,
}

impl server::Server for Handler {
    type Handler = Self;
    fn new_client(&mut self, _: Option<std::net::SocketAddr>) -> Self {
        self.clone()
    }
}

impl server::Handler for Handler {
    type Error = russh::Error;

    async fn auth_password(&mut self, user: &str, password: &str) -> Result<Auth, Self::Error> {
        if user == self.user && password == self.password {
            Ok(Auth::Accept)
        } else {
            Ok(Auth::reject())
        }
    }

    async fn channel_open_session(
        &mut self,
        _channel: Channel<Msg>,
        reply: server::ChannelOpenHandle,
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        reply.accept().await;
        Ok(())
    }

    async fn exec_request(
        &mut self,
        channel: ChannelId,
        data: &[u8],
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        session.channel_success(channel)?;
        let out = format!("{} says {}", self.label, String::from_utf8_lossy(data));
        session.data(channel, out.into_bytes())?;
        session.exit_status_request(channel, 0)?;
        session.eof(channel)?;
        session.close(channel)?;
        Ok(())
    }

    async fn pty_request(
        &mut self,
        channel: ChannelId,
        _term: &str,
        cols: u32,
        rows: u32,
        _pw: u32,
        _ph: u32,
        _modes: &[(russh::Pty, u32)],
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        session.channel_success(channel)?;
        session.data(channel, format!("pty {cols}x{rows}\n").into_bytes())?;
        Ok(())
    }

    async fn shell_request(
        &mut self,
        channel: ChannelId,
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        session.channel_success(channel)?;
        self.shells.push(channel);
        Ok(())
    }

    async fn data(
        &mut self,
        channel: ChannelId,
        data: &[u8],
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        if !self.shells.contains(&channel) {
            return Ok(()); // e.g. a direct-tcpip channel; its bytes belong to the forwarder
        }
        if data == b"exit\n" {
            session.exit_status_request(channel, 3)?;
            session.close(channel)?;
        } else {
            session.data(channel, [b"echo:".as_slice(), data].concat())?;
        }
        Ok(())
    }

    async fn window_change_request(
        &mut self,
        channel: ChannelId,
        cols: u32,
        rows: u32,
        _pw: u32,
        _ph: u32,
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        session.data(channel, format!("resized {cols}x{rows}\n").into_bytes())?;
        Ok(())
    }

    async fn channel_open_direct_tcpip(
        &mut self,
        channel: Channel<Msg>,
        host: &str,
        port: u32,
        _originator: &str,
        _originator_port: u32,
        reply: server::ChannelOpenHandle,
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        let Ok(target) = TcpStream::connect((host, port as u16)).await else {
            return Ok(());
        };
        self.forwards.fetch_add(1, Ordering::SeqCst);
        reply.accept().await;
        tokio::spawn(async move {
            let mut s = channel.into_stream();
            let mut t = target;
            let _ = tokio::io::copy_bidirectional(&mut s, &mut t).await;
        });
        Ok(())
    }
}

impl TestServer {
    /// Start a server that accepts exactly `user`/`password`.
    pub async fn start(label: &str, user: &str, password: &str) -> Self {
        let key =
            russh::keys::PrivateKey::random(&mut rand::rng(), russh::keys::Algorithm::Ed25519)
                .expect("key");
        let host_key_line = key.public_key().to_openssh().expect("openssh");
        let config = Arc::new(server::Config {
            keys: vec![key],
            auth_rejection_time: std::time::Duration::from_millis(0),
            auth_rejection_time_initial: Some(std::time::Duration::from_millis(0)),
            ..Default::default()
        });
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.expect("bind");
        let port = listener.local_addr().expect("addr").port();
        let forwards = Arc::new(AtomicUsize::new(0));
        let mut h = Handler {
            label: label.into(),
            user: user.into(),
            password: password.into(),
            forwards: forwards.clone(),
            shells: Vec::new(),
        };
        tokio::spawn(async move {
            let _ = h.run_on_socket(config, &listener).await;
        });
        Self {
            port,
            forwards,
            host_key_line,
        }
    }
}
