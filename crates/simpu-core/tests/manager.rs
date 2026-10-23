#![allow(clippy::unwrap_used)]
use simpu_core::credentials::{Credentials, StaticProvider};
use simpu_core::manager::{CredentialPromptInfo, Events, Manager, PromptInfo, TransferInfo};
use simpu_core::model::{SecretKind, SecretRef};
use simpu_core::{Connection, Node, Protocol, Tree};
use simpu_testkit::TestServer;
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Default)]
struct Fake {
    data: Mutex<String>,
    closed: Mutex<Vec<String>>,
    prompts: Mutex<Vec<PromptInfo>>,
    progress: Mutex<Vec<TransferInfo>>,
    cred_prompts: Mutex<Vec<CredentialPromptInfo>>,
}

impl Events for Fake {
    fn data(&self, _s: &str, b: Vec<u8>) {
        self.data
            .lock()
            .unwrap()
            .push_str(&String::from_utf8_lossy(&b));
    }
    fn closed(&self, _s: &str, r: String) {
        self.closed.lock().unwrap().push(r);
    }
    fn host_key_prompt(&self, p: PromptInfo) {
        self.prompts.lock().unwrap().push(p);
    }
    fn credential_prompt(&self, p: CredentialPromptInfo) {
        self.cred_prompts.lock().unwrap().push(p);
    }
    fn progress(&self, p: TransferInfo) {
        self.progress.lock().unwrap().push(p);
    }
}

async fn until(what: &str, mut f: impl FnMut() -> bool) {
    for _ in 0..200 {
        if f() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("timed out waiting for {what}");
}

fn secret(item: &str) -> Option<SecretRef> {
    Some(SecretRef {
        provider: "static".into(),
        item: item.into(),
        kind: SecretKind::Password,
    })
}

struct Setup {
    mgr: Arc<Manager<Fake>>,
    events: Arc<Fake>,
    conn: Connection,
    dir: tempfile::TempDir,
    remote: tempfile::TempDir,
}

async fn setup() -> Setup {
    let remote = tempfile::tempdir().unwrap();
    let server =
        TestServer::start_with_sftp("srv", "alice", "pw", Some(remote.path().to_path_buf())).await;
    let mut conn = Connection::new("srv", Protocol::Ssh, "127.0.0.1", "alice");
    conn.port = Some(server.port);
    conn.secret = secret("pw");
    let dir = tempfile::tempdir().unwrap();
    let mut creds = Credentials::default();
    creds.register(Box::new(StaticProvider::new(&[("pw", "pw")])));
    let events = Arc::new(Fake::default());
    let mgr = Arc::new(Manager::new(events.clone(), dir.path().to_path_buf(), creds).unwrap());
    mgr.save_tree(Tree {
        nodes: vec![Node::Connection(conn.clone())],
    })
    .unwrap();
    // keep the server alive for the whole test
    std::mem::forget(server);
    Setup {
        mgr,
        events,
        conn,
        dir,
        remote,
    }
}

fn answer_next_prompt(s: &Setup, trust: bool) -> tokio::task::JoinHandle<()> {
    let (mgr, ev) = (s.mgr.clone(), s.events.clone());
    tokio::spawn(async move {
        until("prompt", || !ev.prompts.lock().unwrap().is_empty()).await;
        let id = ev.prompts.lock().unwrap().remove(0).prompt_id;
        mgr.answer_host_key(&id, trust);
    })
}

#[tokio::test]
async fn open_prompts_once_then_remembers_and_streams_io() {
    let s = setup().await;
    let answer = answer_next_prompt(&s, true);
    s.mgr
        .open("s1", &s.conn.id.to_string(), 80, 24)
        .await
        .unwrap();
    answer.await.unwrap();
    until("pty banner", || {
        s.events.data.lock().unwrap().contains("pty 80x24")
    })
    .await;

    s.mgr.write("s1", b"hi\n".to_vec());
    until("echo", || s.events.data.lock().unwrap().contains("echo:hi")).await;
    s.mgr.resize("s1", 120, 40);
    until("resize", || {
        s.events.data.lock().unwrap().contains("resized 120x40")
    })
    .await;

    // known_hosts persisted, so a second session needs no prompt
    assert!(std::fs::read_to_string(s.dir.path().join("known_hosts"))
        .unwrap()
        .contains("127.0.0.1"));
    s.mgr
        .open("s2", &s.conn.id.to_string(), 80, 24)
        .await
        .unwrap();
    assert!(s.events.prompts.lock().unwrap().is_empty());

    s.mgr.write("s1", b"exit\n".to_vec());
    until("closed", || !s.events.closed.lock().unwrap().is_empty()).await;
    assert_eq!(s.events.closed.lock().unwrap()[0], "exit status 3");
}

#[tokio::test]
async fn rejected_host_key_fails_open() {
    let s = setup().await;
    let answer = answer_next_prompt(&s, false);
    let err = s
        .mgr
        .open("s1", &s.conn.id.to_string(), 80, 24)
        .await
        .unwrap_err();
    answer.await.unwrap();
    assert!(err.contains("host key rejected"), "{err}");
}

#[tokio::test]
async fn upload_reports_progress_and_lists_remote() {
    let s = setup().await;
    let answer = answer_next_prompt(&s, true);
    s.mgr
        .open("s1", &s.conn.id.to_string(), 80, 24)
        .await
        .unwrap();
    answer.await.unwrap();

    let local = s.dir.path().join("report.txt");
    std::fs::write(&local, vec![7u8; 150_000]).unwrap();
    let tid = s
        .mgr
        .upload("s1", local.to_str().unwrap(), ".")
        .await
        .unwrap();
    until("transfer finished", || {
        s.events
            .progress
            .lock()
            .unwrap()
            .iter()
            .any(|p| p.transfer_id == tid && p.finished)
    })
    .await;
    let last = s.events.progress.lock().unwrap().last().unwrap().clone();
    assert_eq!(
        (last.done, last.total, last.error),
        (150_000, 150_000, None)
    );
    assert_eq!(
        std::fs::read(s.remote.path().join("report.txt"))
            .unwrap()
            .len(),
        150_000
    );

    let list = s.mgr.sftp_list("s1", ".").await.unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(
        (list[0].name.as_str(), list[0].is_dir, list[0].size),
        ("report.txt", false, 150_000)
    );

    let dl = tempfile::tempdir().unwrap();
    let tid = s
        .mgr
        .download("s1", "/report.txt", dl.path().to_str().unwrap())
        .await
        .unwrap();
    until("download finished", || {
        s.events
            .progress
            .lock()
            .unwrap()
            .iter()
            .any(|p| p.transfer_id == tid && p.finished)
    })
    .await;
    assert_eq!(
        std::fs::read(dl.path().join("report.txt")).unwrap().len(),
        150_000
    );
}

#[tokio::test]
async fn tree_persists_across_managers() {
    let s = setup().await;
    let events = Arc::new(Fake::default());
    let again = Manager::new(events, s.dir.path().to_path_buf(), Credentials::default()).unwrap();
    assert_eq!(again.tree().connections().len(), 1);
}

#[tokio::test]
async fn unknown_connection_and_missing_vault_item_error_cleanly() {
    let s = setup().await;
    assert!(s.mgr.open("x", &uuid_str(), 80, 24).await.is_err());
    assert!(s.mgr.open("x", "not-a-uuid", 80, 24).await.is_err());
    assert!(s.mgr.upload("nope", "/tmp/x", ".").await.is_err());
}

fn uuid_str() -> String {
    "00000000-0000-0000-0000-000000000000".into()
}

#[tokio::test]
async fn connection_without_secret_asks_for_a_password() {
    let s = setup().await;
    // a second connection to the same server, but with no vault secret
    let mut tree = s.mgr.tree();
    let mut ask = s.conn.clone();
    ask.id = uuid::Uuid::new_v4();
    ask.name = "ask".into();
    ask.secret = None;
    tree.nodes.push(Node::Connection(ask.clone()));
    s.mgr.save_tree(tree).unwrap();

    // wrong password, then cancel -> error
    let (mgr, ev) = (s.mgr.clone(), s.events.clone());
    let task = tokio::spawn(async move {
        until("prompt", || !ev.cred_prompts.lock().unwrap().is_empty()).await;
        let p = ev.cred_prompts.lock().unwrap().remove(0);
        assert_eq!((p.hop.as_str(), p.username.as_str()), ("ask", "alice"));
        mgr.answer_credential(&p.prompt_id, None);
    });
    let err = s
        .mgr
        .open("c1", &ask.id.to_string(), 80, 24)
        .await
        .unwrap_err();
    task.await.unwrap();
    assert!(err.contains("cancelled"), "{err}");

    // right password -> connects (host key prompt first)
    let (mgr, ev) = (s.mgr.clone(), s.events.clone());
    let task = tokio::spawn(async move {
        until("prompt", || !ev.cred_prompts.lock().unwrap().is_empty()).await;
        let p = ev.cred_prompts.lock().unwrap().remove(0);
        mgr.answer_credential(&p.prompt_id, Some("pw".into()));
    });
    let hk = answer_next_prompt(&s, true);
    s.mgr.open("c2", &ask.id.to_string(), 80, 24).await.unwrap();
    task.await.unwrap();
    hk.await.unwrap();
    until("pty banner", || {
        s.events.data.lock().unwrap().contains("pty 80x24")
    })
    .await;
}

#[tokio::test]
async fn imports_ssh_config_into_a_folder() {
    let s = setup().await;
    let cfg = s.dir.path().join("ssh_config");
    std::fs::write(
        &cfg,
        "Host gw\n  HostName gw.example.com\nHost app\n  HostName app.internal\n  ProxyJump gw\n",
    )
    .unwrap();
    assert_eq!(s.mgr.import_ssh_config(&cfg).unwrap(), 2);
    let tree = s.mgr.tree();
    assert_eq!(tree.connections().len(), 3, "original + 2 imported");
    let app = tree
        .connections()
        .into_iter()
        .find(|c| c.name == "app")
        .unwrap();
    assert_eq!(app.jump.len(), 1);
    assert_eq!(tree.find(app.jump[0]).unwrap().name, "gw");
    // persisted
    let again = Manager::new(
        Arc::new(Fake::default()),
        s.dir.path().to_path_buf(),
        Credentials::default(),
    )
    .unwrap();
    assert_eq!(again.tree().connections().len(), 3);
    assert!(s
        .mgr
        .import_ssh_config(&s.dir.path().join("missing"))
        .is_err());
}
