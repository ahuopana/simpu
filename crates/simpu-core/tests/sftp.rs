#![allow(clippy::unwrap_used)] // test helpers

use simpu_core::credentials::Secret;
use simpu_core::hostkeys::KnownHosts;
use simpu_core::sftp::{Progress, SftpError};
use simpu_core::ssh::{always_trust, Hop, HopAuth, SshSession};
use simpu_testkit::TestServer;
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

fn write(root: &std::path::Path, rel: &str, bytes: &[u8]) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, bytes).unwrap();
}

#[tokio::test]
async fn upload_and_download_whole_directory_trees() {
    let remote = tempfile::tempdir().unwrap();
    let local = tempfile::tempdir().unwrap();
    let (_s, session) = connect(remote.path()).await;
    let sftp = session.open_sftp().await.unwrap();
    let cancel = AtomicBool::new(false);

    // local tree: nested dirs, an empty dir, files of different sizes
    let src = local.path().join("project");
    write(&src, "README.md", b"hello");
    write(&src, "src/main.rs", &data(150_000));
    write(&src, "src/deep/er/lib.rs", b"x");
    std::fs::create_dir_all(src.join("empty")).unwrap();
    std::fs::create_dir_all(src.join("empty-too/inner")).unwrap();

    let mut seen: Vec<Progress> = vec![];
    let total = sftp
        .upload_tree(&src, "/project", &cancel, |p| seen.push(p))
        .await
        .unwrap();
    assert_eq!(total, 5 + 150_000 + 1);
    assert_eq!(seen.first().unwrap().done, 0);
    assert_eq!(*seen.last().unwrap(), Progress { done: total, total });
    assert!(
        seen.windows(2).all(|w| w[0].done <= w[1].done),
        "monotonic: {seen:?}"
    );
    assert!(seen.iter().all(|p| p.total == total));
    assert_eq!(
        std::fs::read(remote.path().join("project/src/main.rs")).unwrap(),
        data(150_000)
    );
    assert!(remote.path().join("project/empty").is_dir());
    assert!(remote.path().join("project/empty-too/inner").is_dir());
    assert_eq!(
        std::fs::read(remote.path().join("project/src/deep/er/lib.rs")).unwrap(),
        b"x"
    );

    // uploading again resumes/skips and still succeeds (existing directories are fine)
    sftp.upload_tree(&src, "/project", &cancel, |_| {})
        .await
        .unwrap();

    // and download it back somewhere else
    let dst = local.path().join("copy");
    sftp.download_tree("/project", &dst, &cancel, |_| {})
        .await
        .unwrap();
    assert_eq!(
        std::fs::read(dst.join("src/main.rs")).unwrap(),
        data(150_000)
    );
    assert_eq!(std::fs::read(dst.join("README.md")).unwrap(), b"hello");
    assert!(dst.join("empty").is_dir() && dst.join("empty-too/inner").is_dir());
}

#[tokio::test]
async fn directory_transfer_can_be_cancelled() {
    let remote = tempfile::tempdir().unwrap();
    let local = tempfile::tempdir().unwrap();
    let (_s, session) = connect(remote.path()).await;
    let sftp = session.open_sftp().await.unwrap();
    let src = local.path().join("big");
    for i in 0..4 {
        write(&src, &format!("f{i}.bin"), &data(300_000));
    }
    let cancel = AtomicBool::new(false);
    let err = sftp
        .upload_tree(&src, "/big", &cancel, |p| {
            if p.done > 100_000 {
                cancel.store(true, Ordering::Relaxed);
            }
        })
        .await
        .unwrap_err();
    assert!(matches!(err, SftpError::Cancelled));
    assert!(
        !remote.path().join("big/f3.bin").exists(),
        "later files were not started"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn a_hostile_server_file_name_never_escapes_the_download_folder() {
    let remote = tempfile::tempdir().unwrap();
    let local = tempfile::tempdir().unwrap();
    write(remote.path(), "dir/fine.txt", b"ok");
    // a file whose name contains a backslash (a path separator on Windows)
    write(remote.path(), "dir/..\\..\\evil.txt", b"pwned");
    let (_s, session) = connect(remote.path()).await;
    let sftp = session.open_sftp().await.unwrap();
    let dst = local.path().join("out");
    let err = sftp
        .download_tree("/dir", &dst, &AtomicBool::new(false), |_| {})
        .await
        .unwrap_err();
    assert!(
        matches!(&err, SftpError::UnsafeName(n) if n.contains("evil")),
        "{err}"
    );
    assert!(!local.path().join("evil.txt").exists());
    assert!(
        !dst.join("fine.txt").exists(),
        "nothing is written before the whole listing is vetted"
    );
}
