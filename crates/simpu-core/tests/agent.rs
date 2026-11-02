#![cfg(unix)]
#![allow(clippy::unwrap_used)]
//! ssh-agent authentication against an in-process agent (Unix socket).

use russh::keys::agent::client::AgentClient;
use russh::keys::agent::server::{serve, Agent};
use simpu_core::hostkeys::KnownHosts;
use simpu_core::ssh::{always_trust, AgentEndpoint, ErrorKind, Hop, HopAuth, SshSession};
use simpu_testkit::TestServer;
use std::sync::{Arc, Mutex};

#[derive(Clone)]
struct AllowAll;
impl Agent for AllowAll {}

/// Start an agent on a temp socket holding `keys` (OpenSSH PEM).
async fn agent_with(keys: &[&str]) -> (tempfile::TempDir, String) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("agent.sock");
    let listener = tokio::net::UnixListener::bind(&path).unwrap();
    tokio::spawn(async move {
        let _ = serve(tokio_stream_wrapper(listener), AllowAll).await;
    });
    let sock = path.to_string_lossy().to_string();
    let mut c = AgentClient::connect_uds(&sock).await.unwrap();
    for k in keys {
        let key = russh::keys::decode_secret_key(k, None).unwrap();
        c.add_identity(&key, &[]).await.unwrap();
    }
    (dir, sock)
}

fn tokio_stream_wrapper(
    l: tokio::net::UnixListener,
) -> impl futures::Stream<Item = std::io::Result<tokio::net::UnixStream>> + Unpin {
    Box::pin(futures::stream::unfold(l, |l| async move {
        let r = l.accept().await.map(|(s, _)| s);
        Some((r, l))
    }))
}

fn hop(port: u16, sock: &str, fingerprint: Option<String>) -> Hop {
    Hop {
        name: "agented".into(),
        host: "127.0.0.1".into(),
        port,
        username: "alice".into(),
        auth: HopAuth::Agent {
            endpoint: AgentEndpoint::Path(sock.into()),
            fingerprint,
        },
    }
}

fn known() -> Arc<Mutex<KnownHosts>> {
    Arc::new(Mutex::new(KnownHosts::default()))
}

fn fingerprint_of(pem: &str) -> String {
    russh::keys::decode_secret_key(pem, None)
        .unwrap()
        .public_key()
        .fingerprint(russh::keys::HashAlg::Sha256)
        .to_string()
}

#[tokio::test]
async fn authenticates_with_the_matching_key_from_the_agent() {
    let (other_pem, _) = simpu_testkit::fresh_ed25519();
    let (pem, public) = simpu_testkit::fresh_ed25519();
    let server = TestServer::start_full("srv", "alice", "unused", None, Some(public)).await;
    let (_d, sock) = agent_with(&[&other_pem, &pem]).await; // wrong key first: it must be skipped
    let session = SshSession::connect(&[hop(server.port, &sock, None)], known(), always_trust())
        .await
        .unwrap();
    assert_eq!(
        String::from_utf8_lossy(&session.exec("id").await.unwrap().stdout),
        "srv says id"
    );
}

#[tokio::test]
async fn fingerprint_selects_the_key_and_mismatch_is_explained() {
    let (pem, public) = simpu_testkit::fresh_ed25519();
    let (other, _) = simpu_testkit::fresh_ed25519();
    let server = TestServer::start_full("srv", "alice", "unused", None, Some(public)).await;
    let (_d, sock) = agent_with(&[&other, &pem]).await;
    SshSession::connect(
        &[hop(server.port, &sock, Some(fingerprint_of(&pem)))],
        known(),
        always_trust(),
    )
    .await
    .unwrap();

    let err = SshSession::connect(
        &[hop(server.port, &sock, Some("SHA256:doesnotexist".into()))],
        known(),
        always_trust(),
    )
    .await
    .err()
    .unwrap();
    assert!(
        matches!(&err.kind, ErrorKind::Agent(m) if m.contains("fingerprint")),
        "{err}"
    );

    // the agent only has a key the server does not accept
    let (_d2, sock2) = agent_with(&[&other]).await;
    let err = SshSession::connect(&[hop(server.port, &sock2, None)], known(), always_trust())
        .await
        .err()
        .unwrap();
    assert!(matches!(err.kind, ErrorKind::AuthFailed), "{err}");
}

#[tokio::test]
async fn empty_agent_and_missing_agent_give_clear_errors() {
    let server = TestServer::start_full("srv", "alice", "unused", None, None).await;
    let (_d, sock) = agent_with(&[]).await;
    let err = SshSession::connect(&[hop(server.port, &sock, None)], known(), always_trust())
        .await
        .err()
        .unwrap();
    assert!(
        matches!(&err.kind, ErrorKind::Agent(m) if m.contains("no keys")),
        "{err}"
    );

    let err = SshSession::connect(
        &[hop(server.port, "/nonexistent/agent.sock", None)],
        known(),
        always_trust(),
    )
    .await
    .err()
    .unwrap();
    assert!(
        matches!(&err.kind, ErrorKind::Agent(m) if m.contains("cannot reach")),
        "{err}"
    );
}
