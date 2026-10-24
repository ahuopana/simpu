#![allow(clippy::unwrap_used)]

use simpu_core::credentials::Secret;
use simpu_core::forward::{ForwardManager, ForwardState};
use simpu_core::hostkeys::KnownHosts;
use simpu_core::model::Forward;
use simpu_core::ssh::{always_trust, Hop, HopAuth, SshSession};
use simpu_testkit::net::EchoServer;
use simpu_testkit::TestServer;
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

async fn session(server: &TestServer) -> Arc<SshSession> {
    let hop = Hop {
        name: "srv".into(),
        host: "127.0.0.1".into(),
        port: server.port,
        username: "alice".into(),
        auth: HopAuth::Password(Secret::new("pw")),
    };
    Arc::new(
        SshSession::connect(
            &[hop],
            Arc::new(Mutex::new(KnownHosts::default())),
            always_trust(),
        )
        .await
        .unwrap(),
    )
}

async fn echo_roundtrip(port: u16, msg: &[u8]) -> Vec<u8> {
    let mut s = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    s.write_all(msg).await.unwrap();
    let mut buf = vec![0u8; msg.len()];
    s.read_exact(&mut buf).await.unwrap();
    buf
}

fn listening_port(state: &ForwardState) -> u16 {
    match state {
        ForwardState::Listening { port } => *port,
        other => panic!("not listening: {other:?}"),
    }
}

#[tokio::test]
async fn local_forward_carries_bytes_through_the_ssh_session() {
    let (srv, echo) = (
        TestServer::start("srv", "alice", "pw").await,
        EchoServer::start().await,
    );
    let fm = ForwardManager::new(session(&srv).await);
    let info = fm.start(Forward::local(0, "127.0.0.1", echo.port)).await;
    let port = listening_port(&info.state);
    assert_eq!(echo_roundtrip(port, b"hello tunnel").await, b"hello tunnel");
    assert_eq!(echo_roundtrip(port, b"second").await, b"second");
    assert_eq!(
        srv.forwards.load(std::sync::atomic::Ordering::SeqCst),
        2,
        "went through the server"
    );
    assert_eq!(fm.list()[0].connections, 2);
}

#[tokio::test]
async fn dynamic_forward_speaks_socks5() {
    let (srv, echo) = (
        TestServer::start("srv", "alice", "pw").await,
        EchoServer::start().await,
    );
    let fm = ForwardManager::new(session(&srv).await);
    let port = listening_port(&fm.start(Forward::dynamic(0)).await.state);

    let mut s = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    s.write_all(&[5, 1, 0]).await.unwrap();
    let mut sel = [0u8; 2];
    s.read_exact(&mut sel).await.unwrap();
    assert_eq!(sel, [5, 0]);
    let mut req = vec![5, 1, 0, 1, 127, 0, 0, 1];
    req.extend(echo.port.to_be_bytes());
    s.write_all(&req).await.unwrap();
    let mut rep = [0u8; 10];
    s.read_exact(&mut rep).await.unwrap();
    assert_eq!(rep[..2], [5, 0], "success reply");
    s.write_all(b"via socks").await.unwrap();
    let mut buf = [0u8; 9];
    s.read_exact(&mut buf).await.unwrap();
    assert_eq!(&buf, b"via socks");
}

#[tokio::test]
async fn dynamic_forward_reports_unreachable_targets() {
    let srv = TestServer::start("srv", "alice", "pw").await;
    let fm = ForwardManager::new(session(&srv).await);
    let port = listening_port(&fm.start(Forward::dynamic(0)).await.state);
    // a port nobody listens on
    let dead = std::net::TcpListener::bind(("127.0.0.1", 0))
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let mut s = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    s.write_all(&[5, 1, 0]).await.unwrap();
    let mut sel = [0u8; 2];
    s.read_exact(&mut sel).await.unwrap();
    let mut req = vec![5, 1, 0, 1, 127, 0, 0, 1];
    req.extend(dead.to_be_bytes());
    s.write_all(&req).await.unwrap();
    let mut rep = [0u8; 10];
    s.read_exact(&mut rep).await.unwrap();
    assert_eq!(rep[1], 1, "general failure");
}

#[tokio::test]
async fn remote_forward_makes_the_server_connect_back_to_a_local_service() {
    let (srv, echo) = (
        TestServer::start("srv", "alice", "pw").await,
        EchoServer::start().await,
    );
    let fm = ForwardManager::new(session(&srv).await);
    let info = fm.start(Forward::remote(0, "127.0.0.1", echo.port)).await;
    let server_side_port = listening_port(&info.state);
    assert_ne!(server_side_port, 0);
    assert_eq!(
        echo_roundtrip(server_side_port, b"from the server side").await,
        b"from the server side"
    );
}

#[tokio::test]
async fn port_in_use_is_reported_without_breaking_the_session() {
    let (srv, echo) = (
        TestServer::start("srv", "alice", "pw").await,
        EchoServer::start().await,
    );
    let sess = session(&srv).await;
    let fm = ForwardManager::new(sess.clone());
    let taken = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port = taken.local_addr().unwrap().port();
    let info = fm.start(Forward::local(port, "127.0.0.1", echo.port)).await;
    assert!(
        matches!(&info.state, ForwardState::Failed { reason } if reason.contains("cannot listen")),
        "{:?}",
        info.state
    );
    assert!(sess.exec("still alive").await.is_ok());
    // a good one still works afterwards
    let ok = fm.start(Forward::local(0, "127.0.0.1", echo.port)).await;
    assert_eq!(echo_roundtrip(listening_port(&ok.state), b"x").await, b"x");
    assert_eq!(fm.list().len(), 2);
}

#[tokio::test]
async fn stopping_a_forward_closes_its_listener() {
    let (srv, echo) = (
        TestServer::start("srv", "alice", "pw").await,
        EchoServer::start().await,
    );
    let fm = ForwardManager::new(session(&srv).await);
    let info = fm.start(Forward::local(0, "127.0.0.1", echo.port)).await;
    let port = listening_port(&info.state);
    assert_eq!(echo_roundtrip(port, b"a").await, b"a");
    assert!(fm.stop(info.id).await);
    assert!(!fm.stop(info.id).await, "already stopped");
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    assert!(TcpStream::connect(("127.0.0.1", port)).await.is_err());
    assert!(fm.list().is_empty());
}

#[tokio::test]
async fn invalid_destination_is_a_failed_forward() {
    let srv = TestServer::start("srv", "alice", "pw").await;
    let fm = ForwardManager::new(session(&srv).await);
    let bad = Forward {
        dest_host: None,
        ..Forward::local(0, "h", 1)
    };
    assert!(matches!(
        fm.start(bad).await.state,
        ForwardState::Failed { .. }
    ));
}
