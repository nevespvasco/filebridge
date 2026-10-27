//! filebridge's own key (ADR 0012) against a real SSH server: a key from `generate_key`, its
//! public line in the server's authorized keys, signs in through russh; another key is refused.
//! The server is `rclone serve sftp` (it checks keys like sshd; it runs no shell commands, so the
//! install and remove scripts are tested on their own in `crates/host`). Skipped when rclone or
//! `ssh-keygen` isn't installed.

use std::net::TcpListener;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use filebridge_integration_nextcloud::ssh::generate_key;
use filebridge_integration_nextcloud::{ShellConnector, SshConnector, SshTarget};

struct Server(Child);

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn rclone() -> Option<PathBuf> {
    let local = std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/bin/rclone"));
    let on_path = std::env::var_os("PATH").map(|p| {
        std::env::split_paths(&p)
            .map(|d| d.join("rclone"))
            .collect::<Vec<_>>()
    });
    on_path
        .into_iter()
        .flatten()
        .chain(local)
        .find(|p| p.is_file())
}

#[tokio::test]
async fn a_generated_key_signs_in_and_another_is_refused() {
    let Some(rclone) = rclone() else {
        eprintln!("skipped: rclone not found");
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let host_key = dir.path().join("host_key");
    let made = Command::new("ssh-keygen")
        .args(["-q", "-t", "ed25519", "-N", "", "-f"])
        .arg(&host_key)
        .status();
    if !made.is_ok_and(|s| s.success()) {
        eprintln!("skipped: ssh-keygen not available");
        return;
    }
    let ours = generate_key("filebridge-p-test").unwrap();
    assert!(ours.public.starts_with("ssh-ed25519 ") && ours.public.ends_with(" filebridge-p-test"));
    assert!(ours.pem.contains("BEGIN OPENSSH PRIVATE KEY"));
    let authorized = dir.path().join("authorized_keys");
    std::fs::write(&authorized, format!("restrict {}\n", ours.public)).unwrap();

    let port = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let served = dir.path().join("served");
    std::fs::create_dir_all(&served).unwrap();
    let _server = Server(
        Command::new(&rclone)
            .args(["serve", "sftp", "--config", ""])
            .arg(&served)
            .args(["--addr", &format!("127.0.0.1:{port}"), "--authorized-keys"])
            .arg(&authorized)
            .arg("--key")
            .arg(&host_key)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let deadline = Instant::now() + Duration::from_secs(10);
    while std::net::TcpStream::connect(("127.0.0.1", port)).is_err() {
        assert!(
            Instant::now() < deadline,
            "rclone serve sftp never listened"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    let public = std::fs::read_to_string(host_key.with_extension("pub")).unwrap();
    let mut parts = public.split_whitespace();
    let known = dir.path().join("known_hosts");
    std::fs::write(
        &known,
        format!(
            "[127.0.0.1]:{port} {} {}\n",
            parts.next().unwrap(),
            parts.next().unwrap()
        ),
    )
    .unwrap();

    let target = SshTarget {
        host: "127.0.0.1".into(),
        port,
        user: "alice".into(),
        key_file: None,
        known_hosts_file: Some(known),
        key: None,
        password: None,
    };
    let connector = SshConnector::new();
    assert!(
        connector
            .key_accepted(&target, ours.key.clone())
            .await
            .unwrap()
    );
    let other = generate_key("someone-else").unwrap();
    assert!(!connector.key_accepted(&target, other.key).await.unwrap());
    // A key decoded back from what the keyring keeps is the same key.
    let decoded = russh::keys::decode_secret_key(ours.pem.as_str(), None).unwrap();
    assert!(
        connector
            .key_accepted(&target, std::sync::Arc::new(decoded))
            .await
            .unwrap()
    );
}
