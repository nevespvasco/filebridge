//! The shared `Transport` contract through `NextcloudBulk` over rclone:
//! - against `rclone serve webdav` (generic WebDAV): every file goes through rclone (skipped when
//!   rclone isn't installed);
//! - against a real Nextcloud when `FILEBRIDGE_TEST_NEXTCLOUD_URL` (`…/remote.php/dav/files/<user>/`),
//!   `_USER` and `_PASS` (an app password) are set: small files go by bulk upload. Use a
//!   throwaway server, e.g. `docker run -p 127.0.0.1:8099:80 -e SQLITE_DATABASE=nc
//!   -e NEXTCLOUD_ADMIN_USER=… -e NEXTCLOUD_ADMIN_PASSWORD=… nextcloud:31-apache`. Each run
//!   writes under a new folder `filebridge-test-<unix ms>`.

use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use filebridge_secrets::{MemoryStore, SecretKind, SecretStore, StoreResolver, reference};
use filebridge_transport_api::{
    Backend, Destination, ErrorKind, ProgressEvent, ProgressSink, RemotePath, TransferItem,
    TransferJob, TransferOptions, Transport,
};
use filebridge_transport_nextcloud::NextcloudBulk;
use filebridge_transport_rclone::{Rcd, RcloneTransport};

fn rclone() -> Option<PathBuf> {
    let explicit = std::env::var_os("RCLONE").map(PathBuf::from);
    let on_path: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|p| {
            std::env::split_paths(&p)
                .map(|d| d.join("rclone"))
                .collect()
        })
        .unwrap_or_default();
    let local = std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/bin/rclone"));
    explicit
        .into_iter()
        .chain(on_path)
        .chain(local)
        .find(|p| p.is_file())
}

struct Server(Child);

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// The transport under test, its password kept in a memory keyring.
async fn bulk_over_rclone(binary: &Path, password: &str) -> (NextcloudBulk, Backend) {
    let store = Arc::new(MemoryStore::default());
    let pass_ref = reference("p-nextcloud", SecretKind::AppPassword);
    store.set(&pass_ref, password).unwrap();
    let resolver = Arc::new(StoreResolver(store));
    let rcd = Arc::new(Rcd::start(binary).await.unwrap());
    let rclone = Arc::new(RcloneTransport::new(rcd).with_secrets(resolver.clone()));
    let backend = Backend::Webdav {
        url: String::new(),
        user: String::new(),
        password: Some(pass_ref),
        vendor: Some("nextcloud".into()),
    };
    (NextcloudBulk::over(rclone, resolver), backend)
}

fn with_address(backend: &Backend, url: String, user: String) -> Backend {
    match backend.clone() {
        Backend::Webdav {
            password, vendor, ..
        } => Backend::Webdav {
            url,
            user,
            password,
            vendor,
        },
        other => other,
    }
}

#[tokio::test]
async fn a_server_without_bulk_upload_gets_everything_through_rclone() {
    let Some(binary) = rclone() else {
        eprintln!("skipped: rclone not found");
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let served = dir.path().join("served");
    std::fs::create_dir_all(&served).unwrap();
    let port = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let password = "app-pass-AbCdE-12345";
    let child = Command::new(&binary)
        .args(["serve", "webdav", "--config", ""])
        .arg(&served)
        .args(["--addr", &format!("127.0.0.1:{port}")])
        .args(["--baseurl", "/remote.php/dav/files/alice"])
        .args(["--user", "alice", "--pass", password])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let _server = Server(child);
    let deadline = Instant::now() + Duration::from_secs(10);
    while std::net::TcpStream::connect(("127.0.0.1", port)).is_err() {
        assert!(
            Instant::now() < deadline,
            "rclone serve webdav never listened"
        );
        std::thread::sleep(Duration::from_millis(50));
    }

    let (transport, backend) = bulk_over_rclone(&binary, password).await;
    let url = format!("http://127.0.0.1:{port}/remote.php/dav/files/alice/");
    // Generic WebDAV: rclone's `nextcloud` vendor asks the server for checksums `rclone serve`
    // can't give. Not a Nextcloud, so bulk upload isn't tried.
    let backend = match with_address(&backend, url, "alice".into()) {
        Backend::Webdav {
            url,
            user,
            password,
            ..
        } => Backend::Webdav {
            url,
            user,
            password,
            vendor: None,
        },
        other => other,
    };
    let dest = Destination {
        transport: RcloneTransport::ID.into(),
        backend,
        root: "dest".into(),
    };
    assert_eq!(
        transport.id(),
        RcloneTransport::ID,
        "answers to rclone's id"
    );
    let scratch = dir.path().join("scratch");
    std::fs::create_dir_all(&scratch).unwrap();
    filebridge_transport_api::contract::check(&transport, &dest, &scratch).await;
    filebridge_transport_api::contract::check_move(&transport, &dest, &scratch).await;
    assert!(served.join("dest/a.txt").is_file());
}

#[tokio::test]
async fn small_files_go_by_bulk_upload_to_a_real_nextcloud() {
    let env = |k: &str| std::env::var(format!("FILEBRIDGE_TEST_NEXTCLOUD_{k}")).ok();
    let (Some(url), Some(user), Some(pass)) = (env("URL"), env("USER"), env("PASS")) else {
        eprintln!("skipped: FILEBRIDGE_TEST_NEXTCLOUD_URL, _USER and _PASS are not set");
        return;
    };
    let Some(binary) = rclone() else {
        eprintln!("skipped: rclone not found");
        return;
    };
    let (transport, backend) = bulk_over_rclone(&binary, &pass).await;
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis();
    let dest = Destination {
        transport: RcloneTransport::ID.into(),
        backend: with_address(&backend, url.clone(), user.clone()),
        root: format!("filebridge-test-{stamp}/dest"),
    };
    let dir = tempfile::tempdir().unwrap();
    let scratch = dir.path().join("scratch");
    std::fs::create_dir_all(&scratch).unwrap();
    // Not `check_move`: Nextcloud stores every name in NFC, so its NFD seed can't stay NFD.
    // Moves go to rclone unchanged, and the test above covers them.
    filebridge_transport_api::contract::check(&transport, &dest, &scratch).await;

    // Many small files in new folders, a few already there and identical, one big file: bulk
    // takes the small ones (its stage is announced), rclone the big one.
    let src = dir.path().join("many");
    for i in 0..150 {
        let sub = if i % 2 == 0 { "x/y" } else { "z" };
        std::fs::create_dir_all(src.join(sub)).unwrap();
        std::fs::write(src.join(format!("{sub}/f{i:03}.txt")), format!("file {i}")).unwrap();
    }
    std::fs::write(src.join("big.bin"), vec![7u8; 5 << 20]).unwrap();
    let items: Vec<TransferItem> = (0..150)
        .map(|i| {
            let sub = if i % 2 == 0 { "x/y" } else { "z" };
            let name = format!("{sub}/f{i:03}.txt");
            TransferItem {
                root: 0,
                source: name.clone().into(),
                dest: RemotePath::new(format!("many/{name}")).unwrap(),
                size: format!("file {i}").len() as u64,
                expected_hash: None,
            }
        })
        .chain([TransferItem {
            root: 0,
            source: "big.bin".into(),
            dest: RemotePath::new("many/big.bin").unwrap(),
            size: 5 << 20,
            expected_hash: None,
        }])
        .collect();
    let job = TransferJob {
        id: format!("bulk-{stamp}"),
        source_roots: vec![src.clone()],
        items,
        options: TransferOptions { concurrency: 4 },
    };
    for round in 0..2 {
        let (sink, mut rx) = ProgressSink::channel();
        let sent = transport.transfer(&dest, &job, sink).await.unwrap();
        assert_eq!(sent.failed(), 0, "round {round}: {:?}", sent.items);
        let (mut bulk, mut done) = (false, 0);
        while let Ok(event) = rx.try_recv() {
            match event {
                ProgressEvent::Stage(s) if s == "bulk" => bulk = true,
                ProgressEvent::ItemDone { ok: true, .. } => done += 1,
                _ => {}
            }
        }
        assert!(bulk, "round {round}: small files went by bulk upload");
        assert_eq!(done, 151, "round {round}: ItemDone for every file");
    }
    let listed = transport
        .list(&dest, Some(&RemotePath::new("many/x/y").unwrap()), false)
        .await
        .unwrap();
    assert_eq!(listed.len(), 75);

    // A wrong password stops the send as Auth, before anything is sent.
    let (wrong, backend) = bulk_over_rclone(&binary, "wrong-app-password").await;
    let dest = Destination {
        backend: with_address(&backend, url, user),
        ..dest
    };
    let err = wrong
        .transfer(&dest, &job, ProgressSink::noop())
        .await
        .unwrap_err();
    assert_eq!(err.kind, ErrorKind::Auth, "{err:?}");
    assert!(!err.message.contains("wrong-app-password"));
}
