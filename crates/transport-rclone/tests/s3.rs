//! S3 through rclone's `s3` backend, against a real `rclone serve s3` on 127.0.0.1 with the
//! secret access key kept in the keyring (ADR 0022): the shared contract holds (hashes by
//! download: S3 has only MD5), a folder that holds nothing is `NotFound`, the connection test
//! works without `about`, a wrong secret is an `Auth` error, and no error repeats the secret.
//! Skipped when rclone isn't installed.

use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use filebridge_secrets::{MemoryStore, SecretKind, SecretStore, StoreResolver, reference};
use filebridge_transport_api::{
    Backend, CheckStatus, Destination, ErrorKind, RemotePath, Transport,
};
use filebridge_transport_rclone::{Rcd, RcloneTransport};

const ACCESS_KEY: &str = "FBTESTACCESSKEY";
const SECRET: &str = "s3-secret-AbCdE-12345";

fn rclone() -> Option<PathBuf> {
    let explicit = std::env::var_os("RCLONE").map(PathBuf::from);
    let local = std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/bin/rclone"));
    let on_path = std::env::var_os("PATH").map(|p| {
        std::env::split_paths(&p)
            .map(|d| d.join("rclone"))
            .collect::<Vec<_>>()
    });
    explicit
        .into_iter()
        .chain(on_path.into_iter().flatten())
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

/// `rclone serve s3` of `dir/served` (each folder in it a bucket) on a free port; returns its
/// endpoint.
fn serve(rclone: &Path, dir: &Path) -> Option<(Server, String)> {
    let port = TcpListener::bind("127.0.0.1:0")
        .ok()?
        .local_addr()
        .ok()?
        .port();
    let served = dir.join("served");
    std::fs::create_dir_all(served.join("bucket")).unwrap();
    let child = Command::new(rclone)
        .args(["serve", "s3", "--config", ""])
        .arg(&served)
        .args(["--addr", &format!("127.0.0.1:{port}")])
        .args(["--auth-key", &format!("{ACCESS_KEY},{SECRET}")])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let server = Server(child);
    let deadline = Instant::now() + Duration::from_secs(10);
    while std::net::TcpStream::connect(("127.0.0.1", port)).is_err() {
        assert!(Instant::now() < deadline, "rclone serve s3 never listened");
        std::thread::sleep(Duration::from_millis(50));
    }
    Some((server, format!("http://127.0.0.1:{port}")))
}

#[tokio::test]
async fn s3_with_a_secret_key_from_the_keyring() {
    let Some(binary) = rclone() else {
        eprintln!("skipped: rclone not found");
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let Some((_server, endpoint)) = serve(&binary, dir.path()) else {
        eprintln!("skipped: could not start rclone serve s3");
        return;
    };
    let store = Arc::new(MemoryStore::default());
    let secret_ref = reference("p-s3", SecretKind::AppPassword);
    store.set(&secret_ref, SECRET).unwrap();
    let rcd = Arc::new(Rcd::start(&binary).await.unwrap());
    let transport = RcloneTransport::new(rcd).with_secrets(Arc::new(StoreResolver(store.clone())));
    let dest = Destination {
        transport: "rclone".into(),
        backend: Backend::S3 {
            provider: Some("Rclone".into()),
            endpoint: Some(endpoint),
            region: None,
            bucket: "bucket".into(),
            access_key_id: Some(ACCESS_KEY.into()),
            secret_access_key: Some(secret_ref.clone()),
            path_style: true,
        },
        root: "backup/fotos".into(),
    };

    // No `about` on S3: the root is listed instead; it isn't there yet.
    let report = transport.preflight(&dest).await.unwrap();
    let connect = report.checks.iter().find(|c| c.id == "connect").unwrap();
    assert_ne!(connect.status, CheckStatus::Fail, "{connect:?}");
    assert_eq!(report.free_bytes, None);

    let scratch = dir.path().join("scratch");
    std::fs::create_dir_all(&scratch).unwrap();
    filebridge_transport_api::contract::check(&transport, &dest, &scratch).await;
    filebridge_transport_api::contract::check_move(&transport, &dest, &scratch).await;
    assert!(
        dir.path()
            .join("served/bucket/backup/fotos/a.txt")
            .is_file(),
        "written through S3, inside the bucket's folder"
    );
    let report = transport.preflight(&dest).await.unwrap();
    let connect = report.checks.iter().find(|c| c.id == "connect").unwrap();
    assert_eq!(connect.status, CheckStatus::Ok, "{connect:?}");

    // A folder that holds only folders is there; one that holds nothing isn't.
    let deep = RemotePath::new("deep").unwrap();
    let dirs = transport.list(&dest, Some(&deep), true).await.unwrap();
    assert_eq!(dirs.len(), 1, "{dirs:?}");

    store.set(&secret_ref, "wrong-secret-key").unwrap();
    let err = transport.list(&dest, None, false).await.unwrap_err();
    assert_eq!(err.kind, ErrorKind::Auth, "{err:?}");
    for secret in [SECRET, "wrong-secret-key"] {
        assert!(!err.message.contains(secret), "{}", err.message);
    }
    let report = transport.preflight(&dest).await.unwrap();
    let connect = report.checks.iter().find(|c| c.id == "connect").unwrap();
    assert_eq!(connect.status, CheckStatus::Fail, "{connect:?}");
    assert!(!connect.message.contains("wrong-secret-key"));

    // A secret the keyring doesn't hold: `Auth`, before rclone is asked anything.
    store.delete(&secret_ref).unwrap();
    let err = transport.list(&dest, None, false).await.unwrap_err();
    assert_eq!(err.kind, ErrorKind::Auth, "{err:?}");
}
