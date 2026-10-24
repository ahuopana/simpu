#![allow(clippy::unwrap_used)]
//! Supervised sessions: forwards from the connection, keepalive/drop detection,
//! and the auto / ask / off reconnect policies, through the real `Manager`.

use simpu_core::credentials::{Credentials, StaticProvider};
use simpu_core::forward::ForwardState;
use simpu_core::manager::{
    CredentialPromptInfo, Events, Manager, PromptInfo, ReconnectConfig, SessionState, TransferInfo,
};
use simpu_core::model::{Forward, ReconnectPolicy, SecretKind, SecretRef};
use simpu_core::ssh::ConnectOptions;
use simpu_core::{Connection, Node, Protocol, Tree};
use simpu_testkit::net::{Blackhole, EchoServer};
use simpu_testkit::TestServer;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

#[derive(Default)]
struct Rec {
    data: Mutex<String>,
    states: Mutex<Vec<SessionState>>,
    closed: Mutex<Vec<String>>,
}

impl Events for Rec {
    fn data(&self, _: &str, b: Vec<u8>) {
        self.data
            .lock()
            .unwrap()
            .push_str(&String::from_utf8_lossy(&b));
    }
    fn closed(&self, _: &str, r: String) {
        self.closed.lock().unwrap().push(r);
    }
    fn host_key_prompt(&self, _: PromptInfo) {
        panic!("host key should be pre-trusted in these tests");
    }
    fn credential_prompt(&self, _: CredentialPromptInfo) {}
    fn progress(&self, _: TransferInfo) {}
    fn session_state(&self, _: &str, s: SessionState) {
        self.states.lock().unwrap().push(s);
    }
}

async fn until(what: &str, mut f: impl FnMut() -> bool) {
    for _ in 0..400 {
        if f() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("timed out waiting for {what}");
}

fn free_port() -> u16 {
    std::net::TcpListener::bind(("127.0.0.1", 0))
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

struct Env {
    mgr: Arc<Manager<Rec>>,
    ev: Arc<Rec>,
    conn: Connection,
    _dir: tempfile::TempDir,
}

/// `port` is where the client connects (server or a Blackhole in front of it).
fn env(server: &TestServer, port: u16, tweak: impl FnOnce(&mut Connection)) -> Env {
    let mut conn = Connection::new("srv", Protocol::Ssh, "127.0.0.1", "alice");
    conn.port = Some(port);
    conn.secret = Some(SecretRef {
        provider: "static".into(),
        item: "pw".into(),
        kind: SecretKind::Password,
    });
    tweak(&mut conn);
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("known_hosts"),
        format!("[127.0.0.1]:{port} {}\n", server.host_key_line),
    )
    .unwrap();
    let mut creds = Credentials::default();
    creds.register(Box::new(StaticProvider::new(&[("pw", "pw")])));
    let ev = Arc::new(Rec::default());
    let mgr = Arc::new(Manager::new(ev.clone(), dir.path().to_path_buf(), creds).unwrap());
    mgr.save_tree(Tree {
        nodes: vec![Node::Connection(conn.clone())],
    })
    .unwrap();
    mgr.set_reconnect_config(ReconnectConfig {
        base: Duration::from_millis(50),
        max: Duration::from_millis(200),
        max_attempts: None,
    });
    mgr.set_connect_options(ConnectOptions {
        keepalive_interval: Some(Duration::from_millis(150)),
        keepalive_max: 2,
        connect_timeout: Duration::from_millis(600),
    });
    Env {
        mgr,
        ev,
        conn,
        _dir: dir,
    }
}

async fn echo(port: u16, msg: &[u8]) -> Vec<u8> {
    let mut s = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    s.write_all(msg).await.unwrap();
    let mut buf = vec![0u8; msg.len()];
    tokio::time::timeout(Duration::from_secs(3), s.read_exact(&mut buf))
        .await
        .unwrap()
        .unwrap();
    buf
}

fn count(e: &Env, needle: &str) -> usize {
    e.ev.data.lock().unwrap().matches(needle).count()
}

fn has_state(e: &Env, f: impl Fn(&SessionState) -> bool) -> bool {
    e.ev.states.lock().unwrap().iter().any(f)
}

fn connected_count(e: &Env) -> usize {
    e.ev.states
        .lock()
        .unwrap()
        .iter()
        .filter(|s| **s == SessionState::Connected)
        .count()
}

#[tokio::test]
async fn saved_forward_starts_with_the_session_and_survives_a_drop_with_auto_reconnect() {
    let (srv, target) = (
        TestServer::start("srv", "alice", "pw").await,
        EchoServer::start().await,
    );
    let local = free_port();
    let e = env(&srv, srv.port, |c| {
        c.forwards = vec![Forward::local(local, "127.0.0.1", target.port)]
    });
    assert_eq!(
        e.conn.effective_reconnect(),
        ReconnectPolicy::Auto,
        "forwards imply auto by default"
    );

    e.mgr
        .open("s1", &e.conn.id.to_string(), 80, 24)
        .await
        .unwrap();
    until("shell banner", || count(&e, "pty 80x24") == 1).await;
    assert_eq!(echo(local, b"before").await, b"before");

    srv.kill_connections();
    until("reconnected", || connected_count(&e) == 2).await;
    assert!(has_state(&e, |s| matches!(
        s,
        SessionState::Reconnecting { attempt: 1, .. }
    )));
    assert_eq!(
        echo(local, b"after").await,
        b"after",
        "forward restored on the same port"
    );
    until("second shell banner", || count(&e, "pty 80x24") == 2).await;
    assert_eq!(count(&e, "[connection lost]"), 1);
    assert_eq!(count(&e, "[reconnected]"), 1);
}

#[tokio::test]
async fn ask_policy_waits_for_the_user_then_reconnects() {
    let srv = TestServer::start("srv", "alice", "pw").await;
    let e = env(&srv, srv.port, |_| {});
    assert_eq!(
        e.conn.effective_reconnect(),
        ReconnectPolicy::Ask,
        "plain shell: the user decides"
    );
    e.mgr
        .open("s1", &e.conn.id.to_string(), 80, 24)
        .await
        .unwrap();
    until("banner", || count(&e, "pty 80x24") == 1).await;

    srv.kill_connections();
    until("lost", || {
        has_state(&e, |s| matches!(s, SessionState::Lost { .. }))
    })
    .await;
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert_eq!(connected_count(&e), 1, "no automatic reconnect");
    assert!(e.ev.closed.lock().unwrap().is_empty());

    e.mgr.reconnect("s1");
    until("reconnected", || connected_count(&e) == 2).await;
    e.mgr.write("s1", b"hi\n".to_vec());
    until("shell works again", || count(&e, "echo:hi") == 1).await;
}

#[tokio::test]
async fn off_policy_closes_the_session() {
    let srv = TestServer::start("srv", "alice", "pw").await;
    let e = env(&srv, srv.port, |c| c.reconnect = Some(ReconnectPolicy::Off));
    e.mgr
        .open("s1", &e.conn.id.to_string(), 80, 24)
        .await
        .unwrap();
    until("banner", || count(&e, "pty 80x24") == 1).await;
    srv.kill_connections();
    until("closed", || !e.ev.closed.lock().unwrap().is_empty()).await;
    assert!(has_state(&e, |s| matches!(s, SessionState::Closed { .. })));
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(connected_count(&e), 1);
}

#[tokio::test]
async fn typing_exit_is_a_normal_close_not_a_drop() {
    let srv = TestServer::start("srv", "alice", "pw").await;
    let e = env(&srv, srv.port, |c| {
        c.reconnect = Some(ReconnectPolicy::Auto)
    });
    e.mgr
        .open("s1", &e.conn.id.to_string(), 80, 24)
        .await
        .unwrap();
    until("banner", || count(&e, "pty 80x24") == 1).await;
    e.mgr.write("s1", b"exit\n".to_vec());
    until("closed", || {
        e.ev.closed
            .lock()
            .unwrap()
            .iter()
            .any(|r| r == "exit status 3")
    })
    .await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        connected_count(&e),
        1,
        "a deliberate exit must not reconnect"
    );
}

#[tokio::test]
async fn silent_link_death_is_detected_and_retries_give_up_then_user_can_resume() {
    let srv = TestServer::start("srv", "alice", "pw").await;
    let hole = Blackhole::start(srv.port).await;
    let e = env(&srv, hole.port, |c| {
        c.reconnect = Some(ReconnectPolicy::Auto)
    });
    e.mgr.set_reconnect_config(ReconnectConfig {
        base: Duration::from_millis(30),
        max: Duration::from_millis(60),
        max_attempts: Some(2),
    });
    e.mgr
        .open("s1", &e.conn.id.to_string(), 80, 24)
        .await
        .unwrap();
    until("banner", || count(&e, "pty 80x24") == 1).await;

    hole.pause();
    until("gave up", || {
        has_state(
            &e,
            |s| matches!(s, SessionState::Lost { reason } if reason.contains("gave up")),
        )
    })
    .await;
    assert!(has_state(&e, |s| matches!(
        s,
        SessionState::Reconnecting { attempt: 2, .. }
    )));

    hole.resume();
    e.mgr.reconnect("s1");
    until("reconnected after resume", || connected_count(&e) == 2).await;
}

#[tokio::test]
async fn tunnel_only_session_keeps_forwards_across_reconnects() {
    let (srv, target) = (
        TestServer::start("srv", "alice", "pw").await,
        EchoServer::start().await,
    );
    let local = free_port();
    let e = env(&srv, srv.port, |c| {
        c.no_shell = true;
        c.forwards = vec![Forward::local(local, "127.0.0.1", target.port)];
    });
    e.mgr
        .open("s1", &e.conn.id.to_string(), 80, 24)
        .await
        .unwrap();
    assert_eq!(echo(local, b"tunnel").await, b"tunnel");
    assert_eq!(count(&e, "pty"), 0, "no shell was opened");
    srv.kill_connections();
    until("reconnected", || connected_count(&e) == 2).await;
    assert_eq!(echo(local, b"again").await, b"again");
    assert_eq!(count(&e, "pty"), 0);
}

#[tokio::test]
async fn adhoc_forwards_can_be_added_listed_stopped_and_are_restored() {
    let (srv, target) = (
        TestServer::start("srv", "alice", "pw").await,
        EchoServer::start().await,
    );
    let e = env(&srv, srv.port, |c| {
        c.reconnect = Some(ReconnectPolicy::Auto)
    });
    e.mgr
        .open("s1", &e.conn.id.to_string(), 80, 24)
        .await
        .unwrap();
    assert!(e.mgr.forward_list("s1").unwrap().is_empty());

    let port = free_port();
    let a = e
        .mgr
        .forward_add("s1", Forward::local(port, "127.0.0.1", target.port))
        .await
        .unwrap();
    assert_eq!(a.state, ForwardState::Listening { port });
    assert_eq!(echo(port, b"adhoc").await, b"adhoc");
    assert!(
        e.mgr
            .forward_add("s1", Forward::local(0, "", 1))
            .await
            .is_err(),
        "validated"
    );

    srv.kill_connections();
    until("reconnected", || connected_count(&e) == 2).await;
    assert_eq!(echo(port, b"restored").await, b"restored");
    let list = e.mgr.forward_list("s1").unwrap();
    assert_eq!(
        (list.len(), list[0].id),
        (1, a.id),
        "stable id across reconnects"
    );

    e.mgr.forward_stop("s1", a.id).await.unwrap();
    assert!(e.mgr.forward_list("s1").unwrap().is_empty());
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(TcpStream::connect(("127.0.0.1", port)).await.is_err());
}

#[tokio::test]
async fn a_forward_that_cannot_bind_does_not_break_the_session() {
    let (srv, target) = (
        TestServer::start("srv", "alice", "pw").await,
        EchoServer::start().await,
    );
    let taken = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port = taken.local_addr().unwrap().port();
    let e = env(&srv, srv.port, |c| {
        c.forwards = vec![Forward::local(port, "127.0.0.1", target.port)]
    });
    e.mgr
        .open("s1", &e.conn.id.to_string(), 80, 24)
        .await
        .unwrap();
    until("banner", || count(&e, "pty 80x24") == 1).await;
    let list = e.mgr.forward_list("s1").unwrap();
    assert!(
        matches!(&list[0].state, ForwardState::Failed { reason } if reason.contains("cannot listen"))
    );
}
