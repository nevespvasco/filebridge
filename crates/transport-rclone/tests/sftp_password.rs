//! SFTP with a password kept in the keyring (ADR 0004), against a real `rclone serve sftp` on
//! 127.0.0.1 with a password and a host key in a `known_hosts` file: the shared contract holds,
//! a wrong password is an `Auth` error, and no error repeats the password. Skipped when rclone
//! or `ssh-keygen` isn't installed.

use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use filebridge_secrets::{MemoryStore, SecretKind, SecretStore, reference};
use filebridge_transport_api::{Backend, Destination, ErrorKind, Transport};
use filebridge_transport_rclone::{Rcd, RcloneTransport, Strategy};

const PASSWORD: &str = "correct horse battery staple";

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

/// `rclone serve sftp` on a free port, with a fresh host key; returns the port and the
/// `known_hosts` line for it.
fn serve(rclone: &Path, dir: &Path) -> Option<(Server, u16, PathBuf)> {
    let key = dir.join("host_key");
    let made = Command::new("ssh-keygen")
        .args(["-q", "-t", "ed25519", "-N", "", "-f"])
        .arg(&key)
        .status()
        .ok()?;
    if !made.success() {
        return None;
    }
    let port = TcpListener::bind("127.0.0.1:0")
        .ok()?
        .local_addr()
        .ok()?
        .port();
    let served = dir.join("served");
    std::fs::create_dir_all(&served).unwrap();
    let child = Command::new(rclone)
        .args(["serve", "sftp", "--config", ""])
        .arg(&served)
        .args(["--addr", &format!("127.0.0.1:{port}")])
        .args(["--user", "alice", "--pass", PASSWORD, "--key"])
        .arg(&key)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let server = Server(child);
    let deadline = Instant::now() + Duration::from_secs(10);
    while std::net::TcpStream::connect(("127.0.0.1", port)).is_err() {
        assert!(
            Instant::now() < deadline,
            "rclone serve sftp never listened"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    let public = std::fs::read_to_string(key.with_extension("pub")).unwrap();
    let mut parts = public.split_whitespace();
    let known = dir.join("known_hosts");
    std::fs::write(
        &known,
        format!(
            "[127.0.0.1]:{port} {} {}\n",
            parts.next().unwrap(),
            parts.next().unwrap()
        ),
    )
    .unwrap();
    Some((server, port, known))
}

#[tokio::test]
async fn sftp_with_a_password_from_the_keyring() {
    let Some(binary) = rclone() else {
        eprintln!("skipped: rclone not found");
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let Some((_server, port, known_hosts)) = serve(&binary, dir.path()) else {
        eprintln!("skipped: ssh-keygen not available");
        return;
    };
    let store = Arc::new(MemoryStore::default());
    let pass_ref = reference("p-sftp", SecretKind::AppPassword);
    store.set(&pass_ref, PASSWORD).unwrap();
    let rcd = Arc::new(Rcd::start(&binary).await.unwrap());
    let transport = RcloneTransport::new(rcd, Strategy::PerFile).with_secrets(store.clone());
    let dest = Destination {
        transport: "rclone".into(),
        backend: Backend::Sftp {
            host: "127.0.0.1".into(),
            port: Some(port),
            user: Some("alice".into()),
            key_file: None,
            known_hosts_file: Some(known_hosts),
            password: Some(pass_ref.clone()),
        },
        root: "dest".into(),
    };
    let scratch = dir.path().join("scratch");
    std::fs::create_dir_all(&scratch).unwrap();
    filebridge_transport_api::contract::check(&transport, &dest, &scratch).await;
    filebridge_transport_api::contract::check_move(&transport, &dest, &scratch).await;

    // A wrong password: refused as Auth, and neither password appears in the message.
    store.set(&pass_ref, "wrong-password-here").unwrap();
    let err = transport.list(&dest, None, false).await.unwrap_err();
    assert_eq!(err.kind, ErrorKind::Auth, "{err:?}");
    for secret in [PASSWORD, "wrong-password-here"] {
        assert!(!err.message.contains(secret), "{}", err.message);
    }

    // Gone from the keyring: a clear Auth error, not a silent anonymous attempt.
    store.delete(&pass_ref).unwrap();
    let err = transport.list(&dest, None, false).await.unwrap_err();
    assert_eq!(err.kind, ErrorKind::Auth, "{err:?}");
}
