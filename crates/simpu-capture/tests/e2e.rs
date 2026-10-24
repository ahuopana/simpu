#![allow(clippy::unwrap_used)]

//! Capture wired into the real Manager against the in-process SSH server.

use simpu_capture::{anonymize::AnonymizeOptions, Capture, ExportOptions, Scope};
use simpu_core::credentials::{Credentials, StaticProvider};
use simpu_core::manager::{CredentialPromptInfo, Events, Manager, PromptInfo, TransferInfo};
use simpu_core::model::{SecretKind, SecretRef};
use simpu_core::{Connection, Node, Protocol, Tree};
use simpu_testkit::TestServer;
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Default)]
struct Ui {
    data: Mutex<String>,
    prompts: Mutex<Vec<PromptInfo>>,
}

impl Events for Ui {
    fn data(&self, _: &str, b: Vec<u8>) {
        self.data
            .lock()
            .unwrap()
            .push_str(&String::from_utf8_lossy(&b));
    }
    fn closed(&self, _: &str, _: String) {}
    fn host_key_prompt(&self, p: PromptInfo) {
        self.prompts.lock().unwrap().push(p);
    }
    fn credential_prompt(&self, _: CredentialPromptInfo) {}
    fn progress(&self, _: TransferInfo) {}
}

async fn until(mut f: impl FnMut() -> bool) {
    for _ in 0..200 {
        if f() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("timed out");
}

#[tokio::test]
async fn record_export_last_command_anonymized_without_password() {
    let server = TestServer::start("srv", "alice", "s3cretpw").await;
    let mut conn = Connection::new("web", Protocol::Ssh, "127.0.0.1", "alice");
    conn.port = Some(server.port);
    conn.secret = Some(SecretRef {
        provider: "static".into(),
        item: "pw".into(),
        kind: SecretKind::Password,
    });
    let dir = tempfile::tempdir().unwrap();
    let mut creds = Credentials::default();
    creds.register(Box::new(StaticProvider::new(&[("pw", "s3cretpw")])));
    let ui = Arc::new(Ui::default());
    let mgr = Arc::new(Manager::new(ui.clone(), dir.path().to_path_buf(), creds).unwrap());
    mgr.save_tree(Tree {
        nodes: vec![Node::Connection(conn.clone())],
    })
    .unwrap();
    let cap = Arc::new(Capture::default());
    mgr.register_observer(cap.clone());

    cap.start("s1");
    let (m, u) = (mgr.clone(), ui.clone());
    let hk = tokio::spawn(async move {
        until(|| !u.prompts.lock().unwrap().is_empty()).await;
        let id = u.prompts.lock().unwrap().remove(0).prompt_id;
        m.answer_host_key(&id, true);
    });
    mgr.open("s1", &conn.id.to_string(), 80, 24).await.unwrap();
    hk.await.unwrap();
    until(|| ui.data.lock().unwrap().contains("pty 80x24")).await;

    mgr.write("s1", b"alice@web:~$ ls\n".to_vec());
    mgr.write(
        "s1",
        b"alice@web:~$ login --password s3cretpw as alice\n".to_vec(),
    );
    until(|| ui.data.lock().unwrap().contains("as alice")).await;
    mgr.write("s1", b"alice@web:~$ \n".to_vec());
    until(|| cap.status("s1").bytes > 60).await;
    cap.stop("s1");

    let opts = ExportOptions {
        scope: Scope::LastCommands(1),
        anonymize: Some(AnonymizeOptions {
            usernames: true,
            hostnames: true,
            secrets: true,
            ..Default::default()
        }),
        ..Default::default()
    };
    let e = cap.export("s1", &opts).unwrap();
    assert!(!e.fallback_to_lines);
    // the host already masked the vault password before the extension saw it
    assert!(e.text.contains("login --password ⟦secret⟧"), "{}", e.text);
    assert!(
        !e.text.contains("s3cretpw") && !e.text.contains("alice"),
        "{}",
        e.text
    );
    assert!(e.text.contains("user1"), "{}", e.text);
}
