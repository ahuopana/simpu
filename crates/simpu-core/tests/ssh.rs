mod common;

use common::TestServer;
use simpu_core::credentials::{Credentials, Secret, StaticProvider};
use simpu_core::hostkeys::KnownHosts;
use simpu_core::model::{SecretKind, SecretRef};
use simpu_core::ssh::{
    always_trust, never_trust, plan_hops, ErrorKind, Hop, ShellEvent, SshSession,
};
use simpu_core::{Connection, Node, Protocol, Tree};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};

fn hop(name: &str, port: u16, user: &str, pw: &str) -> Hop {
    Hop {
        name: name.into(),
        host: "127.0.0.1".into(),
        port,
        username: user.into(),
        password: Some(Secret::new(pw)),
    }
}

fn known() -> Arc<Mutex<KnownHosts>> {
    Arc::new(Mutex::new(KnownHosts::default()))
}

#[tokio::test]
async fn exec_over_password_auth() {
    let s = TestServer::start("srv", "alice", "pw").await;
    let session = SshSession::connect(
        &[hop("srv", s.port, "alice", "pw")],
        known(),
        always_trust(),
    )
    .await
    .unwrap();
    let out = session.exec("uname").await.unwrap();
    assert_eq!(String::from_utf8_lossy(&out.stdout), "srv says uname");
    assert_eq!(out.exit_status, Some(0));
}

#[tokio::test]
async fn wrong_password_names_the_hop() {
    let s = TestServer::start("srv", "alice", "pw").await;
    let err = SshSession::connect(
        &[hop("srv", s.port, "alice", "bad")],
        known(),
        always_trust(),
    )
    .await
    .err()
    .unwrap();
    assert!(matches!(err.kind, ErrorKind::AuthFailed));
    assert_eq!((err.index, err.name.as_str()), (0, "srv"));
}

#[tokio::test]
async fn host_key_unknown_rejected_then_trusted_then_changed() {
    let s = TestServer::start("srv", "alice", "pw").await;
    let kh = known();
    let h = || [hop("srv", s.port, "alice", "pw")];

    let err = SshSession::connect(&h(), kh.clone(), never_trust())
        .await
        .err()
        .unwrap();
    assert!(matches!(err.kind, ErrorKind::HostKeyRejected));

    SshSession::connect(&h(), kh.clone(), always_trust())
        .await
        .unwrap();
    // remembered: no prompt needed any more
    SshSession::connect(&h(), kh.clone(), never_trust())
        .await
        .unwrap();

    // a different stored key for the same host:port is refused even if the user would trust
    let bad = Arc::new(Mutex::new(KnownHosts::default()));
    let other = TestServer::start("other", "x", "y").await;
    bad.lock()
        .unwrap()
        .add("127.0.0.1", s.port, &other.host_key_line);
    let err = SshSession::connect(&h(), bad, always_trust())
        .await
        .err()
        .unwrap();
    assert!(matches!(err.kind, ErrorKind::HostKeyChanged));
}

fn secret(item: &str) -> Option<SecretRef> {
    Some(SecretRef {
        provider: "static".into(),
        item: item.into(),
        kind: SecretKind::Password,
    })
}

#[tokio::test]
async fn jump_chain_with_vault_passwords() {
    let jump = TestServer::start("jump", "ops", "jump-pw").await;
    let target = TestServer::start("target", "alice", "target-pw").await;

    let mut j = Connection::new("jump", Protocol::Ssh, "127.0.0.1", "ops");
    j.port = Some(jump.port);
    j.secret = secret("jump");
    let mut t = Connection::new("target", Protocol::Ssh, "127.0.0.1", "alice");
    t.port = Some(target.port);
    t.secret = secret("target");
    t.jump = vec![j.id];
    let tree = Tree {
        nodes: vec![Node::Connection(j), Node::Connection(t.clone())],
    };

    let mut creds = Credentials::default();
    creds.register(Box::new(StaticProvider::new(&[
        ("jump", "jump-pw"),
        ("target", "target-pw"),
    ])));

    let hops = plan_hops(&tree, tree.find(t.id).unwrap(), &creds).unwrap();
    assert_eq!(hops.len(), 2);
    let session = SshSession::connect(&hops, known(), always_trust())
        .await
        .unwrap();
    let out = session.exec("whoami").await.unwrap();
    assert_eq!(String::from_utf8_lossy(&out.stdout), "target says whoami");
    assert_eq!(
        jump.forwards.load(Ordering::SeqCst),
        1,
        "traffic went through the jump host"
    );
}

#[tokio::test]
async fn failure_on_second_hop_is_attributed() {
    let jump = TestServer::start("jump", "ops", "jump-pw").await;
    let target = TestServer::start("target", "alice", "target-pw").await;
    let hops = [
        hop("jump", jump.port, "ops", "jump-pw"),
        hop("target", target.port, "alice", "wrong"),
    ];
    let err = SshSession::connect(&hops, known(), always_trust())
        .await
        .err()
        .unwrap();
    assert_eq!((err.index, err.name.as_str()), (1, "target"));
    assert!(matches!(err.kind, ErrorKind::AuthFailed));
}

#[tokio::test]
async fn locked_vault_blocks_planning() {
    let mut c = Connection::new("t", Protocol::Ssh, "h", "u");
    c.secret = secret("x");
    let tree = Tree {
        nodes: vec![Node::Connection(c.clone())],
    };
    let mut creds = Credentials::default();
    creds.register(Box::new(StaticProvider::new(&[("x", "y")]).locked()));
    assert!(plan_hops(&tree, tree.find(c.id).unwrap(), &creds).is_err());
}

async fn read_until(shell: &mut simpu_core::ssh::Shell, needle: &str) -> String {
    let mut acc = String::new();
    while !acc.contains(needle) {
        match shell.next().await {
            ShellEvent::Data(d) => acc.push_str(&String::from_utf8_lossy(&d)),
            other => panic!("unexpected {other:?} while waiting for {needle:?}, got {acc:?}"),
        }
    }
    acc
}

#[tokio::test]
async fn interactive_shell_echo_resize_exit() {
    let s = TestServer::start("srv", "alice", "pw").await;
    let session = SshSession::connect(
        &[hop("srv", s.port, "alice", "pw")],
        known(),
        always_trust(),
    )
    .await
    .unwrap();
    let mut shell = session.open_shell(80, 24).await.unwrap();
    read_until(&mut shell, "pty 80x24").await;
    shell.write(b"hi\n").await.unwrap();
    read_until(&mut shell, "echo:hi").await;
    shell.resize(120, 40).await.unwrap();
    read_until(&mut shell, "resized 120x40").await;
    shell.write(b"exit\n").await.unwrap();
    assert_eq!(shell.next().await, ShellEvent::Exit(3));
    assert_eq!(shell.next().await, ShellEvent::Closed);
}
