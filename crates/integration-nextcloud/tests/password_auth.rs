//! A phone has no SSH agent: it signs in once with the SSH user's password to install
//! filebridge's own key (ADR 0012). The password goes only to the server, and a wrong one says
//! so. The server here is russh's own, answering as sshd would.

use std::sync::Arc;

use filebridge_integration_nextcloud::ssh::generate_key;
use filebridge_integration_nextcloud::{ShellConnector, SshConnector, SshPassword, SshTarget};
use russh::keys::PrivateKey;
use russh::server::{self, Auth};

/// Takes `alice` with `correct horse` and nothing else.
struct PasswordOnly;

impl server::Handler for PasswordOnly {
    type Error = russh::Error;

    async fn auth_password(&mut self, user: &str, password: &str) -> Result<Auth, Self::Error> {
        Ok(if user == "alice" && password == "correct horse" {
            Auth::Accept
        } else {
            Auth::reject()
        })
    }
}

#[tokio::test]
async fn the_password_signs_in_and_a_wrong_one_is_named() {
    let host_key = generate_key("host").unwrap().key;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let config = Arc::new(server::Config {
        keys: vec![PrivateKey::clone(&host_key)],
        auth_rejection_time: std::time::Duration::from_millis(1),
        auth_rejection_time_initial: Some(std::time::Duration::ZERO),
        ..Default::default()
    });
    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            if let Ok(session) = server::run_stream(config.clone(), stream, PasswordOnly).await {
                tokio::spawn(session);
            }
        }
    });

    let dir = tempfile::tempdir().unwrap();
    let known = dir.path().join("known_hosts");
    std::fs::write(
        &known,
        format!(
            "[127.0.0.1]:{port} {}\n",
            host_key.public_key().to_openssh().unwrap()
        ),
    )
    .unwrap();
    let target = |password: &str| SshTarget {
        host: "127.0.0.1".into(),
        port,
        user: "alice".into(),
        key_file: None,
        known_hosts_file: Some(known.clone()),
        key: None,
        password: Some(SshPassword::new(password)),
    };
    let connector = SshConnector::new();
    assert!(connector.connect(&target("correct horse")).await.is_ok());
    let wrong = connector.connect(&target("wrong")).await.err().unwrap();
    assert!(
        wrong
            .message
            .contains("refused the password for user alice"),
        "{}",
        wrong.message
    );
    // Neither the target's Debug nor its saved form carries the password.
    let t = target("correct horse");
    assert!(!format!("{t:?}").contains("horse"));
    assert!(!serde_json::to_string(&t).unwrap().contains("horse"));
}
