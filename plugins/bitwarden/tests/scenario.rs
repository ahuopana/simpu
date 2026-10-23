//! Headline scenario: connect to a target through a jump host, with both
//! passwords fetched from Bitwarden (fake `bw`) via the real plugin process,
//! driven through the same `Manager` the GUI uses.

use simpu_core::credentials::Credentials;
use simpu_core::manager::{CredentialPromptInfo, Events, Manager, PromptInfo, TransferInfo};
use simpu_core::model::{SecretKind, SecretRef};
use simpu_core::plugin::PluginProvider;
use simpu_core::{Connection, Node, Protocol, Tree};
use simpu_testkit::TestServer;
use std::path::Path;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Default)]
struct Fake {
    data: Mutex<String>,
    prompts: Mutex<Vec<PromptInfo>>,
}

impl Events for Fake {
    fn data(&self, _: &str, b: Vec<u8>) {
        self.data
            .lock()
            .expect("lock")
            .push_str(&String::from_utf8_lossy(&b));
    }
    fn closed(&self, _: &str, _: String) {}
    fn host_key_prompt(&self, p: PromptInfo) {
        self.prompts.lock().expect("lock").push(p);
    }
    fn credential_prompt(&self, _: CredentialPromptInfo) {}
    fn progress(&self, _: TransferInfo) {}
}

fn bw_item(item: &str) -> Option<SecretRef> {
    Some(SecretRef {
        provider: "bitwarden".into(),
        item: item.into(),
        kind: SecretKind::Password,
    })
}

fn credentials(status: &str) -> Credentials {
    let plugin = PluginProvider::spawn(
        Path::new(env!("CARGO_BIN_EXE_simpu-plugin-bitwarden")),
        &[
            ("BW_BIN", env!("CARGO_BIN_EXE_fake-bw")),
            ("FAKE_BW_STATUS", status),
            ("FAKE_BW_ITEMS", "jump-item=jump-pw;target-item=target-pw"),
        ],
    )
    .expect("plugin starts");
    let mut c = Credentials::default();
    c.register(Box::new(plugin));
    c
}

async fn setup(
    status: &str,
) -> (
    Arc<Manager<Fake>>,
    Arc<Fake>,
    Connection,
    TestServer,
    tempfile::TempDir,
) {
    let jump = TestServer::start("jump", "ops", "jump-pw").await;
    let target = TestServer::start("target", "alice", "target-pw").await;
    let mut j = Connection::new("jump", Protocol::Ssh, "127.0.0.1", "ops");
    j.port = Some(jump.port);
    j.secret = bw_item("jump-item");
    let mut t = Connection::new("target", Protocol::Ssh, "127.0.0.1", "alice");
    t.port = Some(target.port);
    t.secret = bw_item("target-item");
    t.jump = vec![j.id];
    let dir = tempfile::tempdir().expect("tmp");
    let events = Arc::new(Fake::default());
    let mgr = Arc::new(
        Manager::new(
            events.clone(),
            dir.path().to_path_buf(),
            credentials(status),
        )
        .expect("manager"),
    );
    mgr.save_tree(Tree {
        nodes: vec![Node::Connection(j), Node::Connection(t.clone())],
    })
    .expect("save");
    (mgr, events, t, jump, dir)
}

async fn wait(mut f: impl FnMut() -> bool) {
    for _ in 0..200 {
        if f() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("timed out");
}

#[tokio::test]
async fn jump_host_with_bitwarden_passwords() {
    let (mgr, events, target, jump, _dir) = setup("unlocked").await;
    // Trust every host key the user is asked about (jump host and target).
    let (m, e) = (mgr.clone(), events.clone());
    let answerer = tokio::spawn(async move {
        let mut answered = 0;
        while answered < 2 {
            let next = e.prompts.lock().expect("lock").pop();
            match next {
                Some(p) => {
                    m.answer_host_key(&p.prompt_id, true);
                    answered += 1;
                }
                None => tokio::time::sleep(Duration::from_millis(10)).await,
            }
        }
    });
    mgr.open("s1", &target.id.to_string(), 100, 30)
        .await
        .expect("open through jump host");
    answerer.await.expect("answerer");
    wait(|| events.data.lock().expect("lock").contains("pty 100x30")).await;
    assert_eq!(
        jump.forwards.load(Ordering::SeqCst),
        1,
        "target reached via the jump host"
    );
}

#[tokio::test]
async fn locked_vault_fails_before_any_network_traffic() {
    let (mgr, events, target, jump, _dir) = setup("locked").await;
    let err = mgr
        .open("s1", &target.id.to_string(), 80, 24)
        .await
        .expect_err("must fail");
    assert!(err.to_lowercase().contains("locked"), "{err}");
    assert!(events.prompts.lock().expect("lock").is_empty());
    assert_eq!(jump.forwards.load(Ordering::SeqCst), 0);
}
