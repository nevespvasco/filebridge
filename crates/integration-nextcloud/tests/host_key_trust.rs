//! Trusting a server's host key from the app (ADR 0015) against a real SSH server
//! (`rclone serve sftp` on 127.0.0.1): an unknown host is scanned, trusted only with the
//! fingerprint it presents, and signs in afterwards; a host whose key changed is never trusted
//! and its `known_hosts` is left as it was. Skipped when rclone or `ssh-keygen` isn't installed.

use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use filebridge_integration_nextcloud::known_hosts::Standing;
use filebridge_integration_nextcloud::ssh::{
    TrustError, generate_key, scan_host_key, trust_host_key,
};
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

fn keygen(path: &Path) -> bool {
    Command::new("ssh-keygen")
        .args(["-q", "-t", "ed25519", "-N", "", "-f"])
        .arg(path)
        .status()
        .is_ok_and(|s| s.success())
}

/// `rclone serve sftp` with `host_key`, accepting `authorized`; its port.
fn serve(rclone: &Path, dir: &Path, host_key: &Path, authorized: &Path) -> (Server, u16) {
    let port = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let served = dir.join("served");
    std::fs::create_dir_all(&served).unwrap();
    let server = Server(
        Command::new(rclone)
            .args(["serve", "sftp", "--config", ""])
            .arg(&served)
            .args(["--addr", &format!("127.0.0.1:{port}"), "--authorized-keys"])
            .arg(authorized)
            .arg("--key")
            .arg(host_key)
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
    (server, port)
}

/// `ssh-ed25519 AAAA…` of a `.pub` file, without its comment.
fn public_line(pub_file: &Path) -> String {
    let text = std::fs::read_to_string(pub_file).unwrap();
    let mut parts = text.split_whitespace();
    format!("{} {}", parts.next().unwrap(), parts.next().unwrap())
}

#[tokio::test]
async fn an_unknown_host_is_trusted_only_with_its_fingerprint_and_then_signs_in() {
    let Some(rclone) = rclone() else {
        eprintln!("skipped: rclone not found");
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let host_key = dir.path().join("host_key");
    if !keygen(&host_key) {
        eprintln!("skipped: ssh-keygen not available");
        return;
    }
    let ours = generate_key("filebridge-p-test").unwrap();
    let authorized = dir.path().join("authorized_keys");
    std::fs::write(&authorized, format!("{}\n", ours.public)).unwrap();
    let (_server, port) = serve(&rclone, dir.path(), &host_key, &authorized);

    // A phone's first time: no ~/.ssh at all.
    let known = dir.path().join("home/.ssh/known_hosts");
    let target = SshTarget {
        host: "127.0.0.1".into(),
        port,
        user: "alice".into(),
        key_file: None,
        known_hosts_file: Some(known.clone()),
        key: None,
    };
    let connector = SshConnector::new();
    let refused = connector
        .key_accepted(&target, ours.key.clone())
        .await
        .unwrap_err();
    assert!(refused.message.contains("is not in"), "{refused}");

    let scan = scan_host_key("127.0.0.1", port, Some(&known))
        .await
        .unwrap();
    assert_eq!(scan.standing, Standing::Unknown);
    let key = scan.key.unwrap();
    let fingerprint = key.fingerprint(russh::keys::HashAlg::Sha256).to_string();
    assert!(fingerprint.starts_with("SHA256:"));
    let expected_line = public_line(&host_key.with_extension("pub"));
    assert_eq!(key.to_openssh().unwrap().trim_end(), expected_line.as_str());
    assert!(!known.exists(), "scanning writes nothing");

    // Another fingerprint than the one the server presents: nothing is trusted.
    let other = generate_key("x").unwrap();
    let wrong = other
        .key
        .public_key()
        .fingerprint(russh::keys::HashAlg::Sha256)
        .to_string();
    let e = trust_host_key("127.0.0.1", port, Some(&known), &wrong)
        .await
        .unwrap_err();
    assert!(
        matches!(&e, TrustError::Refused(m) if m.contains("not the one you checked")),
        "{e}"
    );
    assert!(!known.exists());

    let trusted = trust_host_key("127.0.0.1", port, Some(&known), &fingerprint)
        .await
        .unwrap();
    assert_eq!(trusted.standing, Standing::Known);
    assert_eq!(
        std::fs::read_to_string(&known).unwrap(),
        format!("[127.0.0.1]:{port} {expected_line}\n")
    );
    assert!(
        connector
            .key_accepted(&target, ours.key.clone())
            .await
            .unwrap()
    );
    // Again: already known, the file is unchanged.
    trust_host_key("127.0.0.1", port, Some(&known), &fingerprint)
        .await
        .unwrap();
    assert_eq!(std::fs::read_to_string(&known).unwrap().lines().count(), 1);
}

#[tokio::test]
async fn a_changed_host_key_is_never_trusted() {
    let Some(rclone) = rclone() else {
        eprintln!("skipped: rclone not found");
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let (host_key, old_key) = (dir.path().join("host_key"), dir.path().join("old_key"));
    if !keygen(&host_key) || !keygen(&old_key) {
        eprintln!("skipped: ssh-keygen not available");
        return;
    }
    // rclone won't start without a key to accept.
    let authorized = dir.path().join("authorized_keys");
    let anyone = generate_key("filebridge-p-test").unwrap();
    std::fs::write(&authorized, format!("{}\n", anyone.public)).unwrap();
    let (_server, port) = serve(&rclone, dir.path(), &host_key, &authorized);

    // known_hosts remembers another key for this server.
    let known = dir.path().join("known_hosts");
    let before = format!(
        "# a comment\n[127.0.0.1]:{port} {}\n",
        public_line(&old_key.with_extension("pub"))
    );
    std::fs::write(&known, &before).unwrap();

    let scan = scan_host_key("127.0.0.1", port, Some(&known))
        .await
        .unwrap();
    assert_eq!(scan.standing, Standing::Changed { line: 2 });
    let presented = scan
        .key
        .unwrap()
        .fingerprint(russh::keys::HashAlg::Sha256)
        .to_string();
    let e = trust_host_key("127.0.0.1", port, Some(&known), &presented)
        .await
        .unwrap_err();
    let TrustError::Refused(m) = &e else {
        panic!("refused, not failed: {e}");
    };
    assert!(m.contains("does not match") && m.contains("line 2"), "{m}");
    assert_eq!(std::fs::read_to_string(&known).unwrap(), before);
}
