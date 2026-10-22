//! Nextcloud's WebDAV mode, through rclone's `webdav` backend, against a real `rclone serve webdav`
//! on 127.0.0.1 with a password kept in the keyring: the shared contract holds (hashes by
//! download, as on Nextcloud, which can't hash SHA-256 itself), a wrong password is an `Auth`
//! error, and no error repeats the password. Skipped when rclone isn't installed.

use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use filebridge_secrets::{MemoryStore, SecretKind, SecretStore, reference};
use filebridge_transport_api::{Backend, Destination, ErrorKind, Transport};
use filebridge_transport_rclone::{Rcd, RcloneTransport, Strategy};

const PASSWORD: &str = "app-pass-AbCdE-12345";

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

/// `rclone serve webdav` on a free port; returns its base URL.
fn serve(rclone: &Path, dir: &Path) -> Option<(Server, String)> {
    let port = TcpListener::bind("127.0.0.1:0")
        .ok()?
        .local_addr()
        .ok()?
        .port();
    let served = dir.join("served");
    std::fs::create_dir_all(&served).unwrap();
    let child = Command::new(rclone)
        .args(["serve", "webdav", "--config", ""])
        .arg(&served)
        .args(["--addr", &format!("127.0.0.1:{port}")])
        .args(["--user", "alice", "--pass", PASSWORD])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let server = Server(child);
    let deadline = Instant::now() + Duration::from_secs(10);
    while std::net::TcpStream::connect(("127.0.0.1", port)).is_err() {
        assert!(
            Instant::now() < deadline,
            "rclone serve webdav never listened"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    Some((server, format!("http://127.0.0.1:{port}/")))
}

#[tokio::test]
async fn webdav_with_a_password_from_the_keyring() {
    let Some(binary) = rclone() else {
        eprintln!("skipped: rclone not found");
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let Some((_server, url)) = serve(&binary, dir.path()) else {
        eprintln!("skipped: could not start rclone serve webdav");
        return;
    };
    let store = Arc::new(MemoryStore::default());
    let pass_ref = reference("p-webdav", SecretKind::AppPassword);
    store.set(&pass_ref, PASSWORD).unwrap();
    let rcd = Arc::new(Rcd::start(&binary).await.unwrap());
    let transport = RcloneTransport::new(rcd, Strategy::PerFile).with_secrets(store.clone());
    let dest = Destination {
        transport: "rclone".into(),
        backend: Backend::Webdav {
            url,
            user: "alice".into(),
            password: Some(pass_ref.clone()),
            vendor: None,
        },
        root: "dest".into(),
    };
    let scratch = dir.path().join("scratch");
    std::fs::create_dir_all(&scratch).unwrap();
    filebridge_transport_api::contract::check(&transport, &dest, &scratch).await;
    filebridge_transport_api::contract::check_move(&transport, &dest, &scratch).await;
    assert!(
        dir.path().join("served/dest/a.txt").is_file(),
        "written through WebDAV"
    );

    store.set(&pass_ref, "wrong-app-password").unwrap();
    let err = transport.list(&dest, None, false).await.unwrap_err();
    assert_eq!(err.kind, ErrorKind::Auth, "{err:?}");
    for secret in [PASSWORD, "wrong-app-password"] {
        assert!(!err.message.contains(secret), "{}", err.message);
    }
}
