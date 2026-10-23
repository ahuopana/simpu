#![allow(clippy::unwrap_used)] // test helpers

mod common;

use common::TestServer;
use simpu_core::credentials::Secret;
use simpu_core::hostkeys::KnownHosts;
use simpu_core::sftp::{Progress, SftpError};
use simpu_core::ssh::{always_trust, Hop, HopAuth, SshSession};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

async fn connect(root: &std::path::Path) -> (TestServer, SshSession) {
    let s = TestServer::start_with_sftp("srv", "alice", "pw", Some(root.to_path_buf())).await;
    let hop = Hop {
        name: "srv".into(),
        host: "127.0.0.1".into(),
        port: s.port,
        username: "alice".into(),
        auth: HopAuth::Password(Secret::new("pw")),
    };
    let session = SshSession::connect(
        &[hop],
        Arc::new(Mutex::new(KnownHosts::default())),
        always_trust(),
    )
    .await
    .unwrap();
    (s, session)
}

fn data(n: usize) -> Vec<u8> {
    (0..n).map(|i| (i % 251) as u8).collect()
}

#[tokio::test]
async fn list_orders_dirs_first() {
    let remote = tempfile::tempdir().unwrap();
    std::fs::create_dir(remote.path().join("zdir")).unwrap();
    std::fs::write(remote.path().join("a.txt"), b"hello").unwrap();
    let (_s, session) = connect(remote.path()).await;
    let sftp = session.open_sftp().await.unwrap();
    let list = sftp.list("/").await.unwrap();
    let names: Vec<_> = list
        .iter()
        .map(|e| (e.name.as_str(), e.is_dir, e.size))
        .collect();
    assert_eq!(names, [("zdir", true, names[0].2), ("a.txt", false, 5)]);
}

#[tokio::test]
async fn upload_and_download_round_trip_with_progress() {
    let remote = tempfile::tempdir().unwrap();
    let local = tempfile::tempdir().unwrap();
    let payload = data(300_000);
    std::fs::write(local.path().join("up.bin"), &payload).unwrap();
    let (_s, session) = connect(remote.path()).await;
    let sftp = session.open_sftp().await.unwrap();
    let cancel = AtomicBool::new(false);

    let mut seen: Vec<Progress> = vec![];
    let n = sftp
        .upload(
            &local.path().join("up.bin"),
            "/up.bin",
            false,
            &cancel,
            |p| seen.push(p),
        )
        .await
        .unwrap();
    assert_eq!(n, 300_000);
    assert_eq!(
        std::fs::read(remote.path().join("up.bin")).unwrap(),
        payload
    );
    assert_eq!(seen.first().unwrap().done, 0);
    assert_eq!(
        *seen.last().unwrap(),
        Progress {
            done: 300_000,
            total: 300_000
        }
    );
    assert!(seen.windows(2).all(|w| w[0].done <= w[1].done));

    sftp.download(
        "/up.bin",
        &local.path().join("down.bin"),
        false,
        &cancel,
        |_| {},
    )
    .await
    .unwrap();
    assert_eq!(
        std::fs::read(local.path().join("down.bin")).unwrap(),
        payload
    );
}

#[tokio::test]
async fn resume_download_and_upload() {
    let remote = tempfile::tempdir().unwrap();
    let local = tempfile::tempdir().unwrap();
    let payload = data(200_000);
    let cancel = AtomicBool::new(false);
    let (_s, session) = connect(remote.path()).await;
    let sftp = session.open_sftp().await.unwrap();

    // download: 70_000 bytes already present locally
    std::fs::write(remote.path().join("f.bin"), &payload).unwrap();
    std::fs::write(local.path().join("f.bin"), &payload[..70_000]).unwrap();
    let mut first = None;
    sftp.download("/f.bin", &local.path().join("f.bin"), true, &cancel, |p| {
        first.get_or_insert(p);
    })
    .await
    .unwrap();
    assert_eq!(
        first.unwrap().done,
        70_000,
        "started where the partial file ended"
    );
    assert_eq!(std::fs::read(local.path().join("f.bin")).unwrap(), payload);

    // upload: 50_000 bytes already on the server
    std::fs::write(local.path().join("g.bin"), &payload).unwrap();
    std::fs::write(remote.path().join("g.bin"), &payload[..50_000]).unwrap();
    sftp.upload(&local.path().join("g.bin"), "/g.bin", true, &cancel, |_| {})
        .await
        .unwrap();
    assert_eq!(std::fs::read(remote.path().join("g.bin")).unwrap(), payload);
}

#[tokio::test]
async fn cancel_stops_transfer() {
    let remote = tempfile::tempdir().unwrap();
    let local = tempfile::tempdir().unwrap();
    std::fs::write(local.path().join("big.bin"), data(1_000_000)).unwrap();
    let (_s, session) = connect(remote.path()).await;
    let sftp = session.open_sftp().await.unwrap();
    let cancel = AtomicBool::new(false);
    let err = sftp
        .upload(
            &local.path().join("big.bin"),
            "/big.bin",
            false,
            &cancel,
            |p| {
                if p.done > 100_000 {
                    cancel.store(true, Ordering::Relaxed);
                }
            },
        )
        .await
        .unwrap_err();
    assert!(matches!(err, SftpError::Cancelled));
    let partial = std::fs::metadata(remote.path().join("big.bin"))
        .unwrap()
        .len();
    assert!(partial < 1_000_000);
}

#[tokio::test]
async fn mkdir_rename_remove() {
    let remote = tempfile::tempdir().unwrap();
    std::fs::write(remote.path().join("a"), b"x").unwrap();
    let (_s, session) = connect(remote.path()).await;
    let sftp = session.open_sftp().await.unwrap();
    sftp.mkdir("/d").await.unwrap();
    sftp.rename("/a", "/d/b").await.unwrap();
    assert!(remote.path().join("d/b").exists());
    sftp.remove_file("/d/b").await.unwrap();
    assert!(!remote.path().join("d/b").exists());
    assert!(sftp.home().await.is_ok());
}
