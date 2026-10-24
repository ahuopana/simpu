//! Jump host chain with credentials from KeePassXC: a password for the jump
//! host and a private key (entry attachment) for the target, through the real
//! plugin process, a fake `keepassxc-cli`, and the same `Manager` the GUI uses.

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
    hostkeys: Mutex<Vec<PromptInfo>>,
    unlocks: Mutex<Vec<CredentialPromptInfo>>,
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
        self.hostkeys.lock().expect("lock").push(p);
    }
    fn credential_prompt(&self, p: CredentialPromptInfo) {
        self.unlocks.lock().expect("lock").push(p);
    }
    fn progress(&self, _: TransferInfo) {}
}

fn secret(item: &str, kind: SecretKind) -> Option<SecretRef> {
    Some(SecretRef {
        provider: "keepassxc".into(),
        item: item.into(),
        kind,
    })
}

async fn wait(mut f: impl FnMut() -> bool) {
    for _ in 0..300 {
        if f() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("timed out");
}

#[tokio::test]
async fn jump_password_and_key_attachment_from_keepassxc_with_in_app_unlock() {
    let key = russh_key();
    let jump = TestServer::start("jump", "ops", "jump-pw").await;
    let target =
        TestServer::start_full("target", "alice", "unused", None, Some(key.public.clone())).await;

    let mut j = Connection::new("jump", Protocol::Ssh, "127.0.0.1", "ops");
    j.port = Some(jump.port);
    j.secret = secret("Servers/jump", SecretKind::Password);
    let mut t = Connection::new("target", Protocol::Ssh, "127.0.0.1", "alice");
    t.port = Some(target.port);
    t.secret = secret("Servers/target#id_ed25519", SecretKind::KeyFile);
    t.jump = vec![j.id];

    let entries = format!(
        "Servers/jump:Password=jump-pw;Servers/target:att:id_ed25519={}",
        key.pem
    );
    let plugin = PluginProvider::spawn(
        Path::new(env!("CARGO_BIN_EXE_simpu-plugin-keepassxc")),
        &[
            ("KEEPASSXC_CLI", env!("CARGO_BIN_EXE_fake-keepassxc-cli")),
            ("KEEPASSXC_DB", "/vault/db.kdbx"),
            ("FAKE_KP_PASSWORD", "master"),
            ("FAKE_KP_ENTRIES", entries.as_str()),
        ],
    )
    .expect("plugin starts");
    let mut creds = Credentials::default();
    creds.register(Box::new(plugin));

    let dir = tempfile::tempdir().expect("tmp");
    let events = Arc::new(Fake::default());
    let mgr =
        Arc::new(Manager::new(events.clone(), dir.path().to_path_buf(), creds).expect("manager"));
    mgr.save_tree(Tree {
        nodes: vec![Node::Connection(j), Node::Connection(t.clone())],
    })
    .expect("save");

    // Answer: wrong master password first, then the right one; trust both host keys.
    let (m, e) = (mgr.clone(), events.clone());
    let driver = tokio::spawn(async move {
        let mut passwords = vec!["wrong", "master"].into_iter();
        let mut trusted = 0;
        let mut seen_errors = Vec::new();
        while trusted < 2 {
            let unlock = e.unlocks.lock().expect("lock").pop();
            if let Some(p) = unlock {
                assert_eq!(
                    (p.kind.as_str(), p.provider.as_deref()),
                    ("unlock", Some("keepassxc"))
                );
                seen_errors.push(p.error.clone());
                m.answer_credential(&p.prompt_id, passwords.next().map(String::from));
            }
            let hk = e.hostkeys.lock().expect("lock").pop();
            if let Some(p) = hk {
                m.answer_host_key(&p.prompt_id, true);
                trusted += 1;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        seen_errors
    });

    mgr.open("s1", &t.id.to_string(), 90, 20)
        .await
        .expect("open via jump host with KeePassXC secrets");
    let errors = driver.await.expect("driver");
    assert_eq!(errors, [None, Some("Wrong master password".to_string())]);
    wait(|| events.data.lock().expect("lock").contains("pty 90x20")).await;
    assert_eq!(jump.forwards.load(Ordering::SeqCst), 1);
}

struct Key {
    pem: String,
    public: String,
}

fn russh_key() -> Key {
    // Same API the testkit uses; kept local so the plugin crate has no direct russh dependency.
    let (pem, public) = simpu_testkit::fresh_ed25519();
    Key { pem, public }
}
