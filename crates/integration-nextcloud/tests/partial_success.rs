//! A server with `AuthenticationMethods publickey,password` takes a key as the first factor and
//! answers "partial success". `key_accepted` must call that accepted: Remove access's proof reads
//! "refused" as "access is gone". The server here is russh's own, answering as sshd would.

use std::sync::Arc;

use filebridge_integration_nextcloud::ssh::generate_key;
use filebridge_integration_nextcloud::{ShellConnector, SshConnector, SshTarget};
use russh::keys::{PrivateKey, PublicKey};
use russh::server::{self, Auth};
use russh::{MethodKind, MethodSet};

/// Takes `first_factor` as one factor and asks for a password next; refuses any other key.
struct TwoFactor {
    first_factor: PublicKey,
}

impl server::Handler for TwoFactor {
    type Error = russh::Error;

    async fn auth_publickey(&mut self, _user: &str, key: &PublicKey) -> Result<Auth, Self::Error> {
        Ok(if key.key_data() == self.first_factor.key_data() {
            Auth::Reject {
                proceed_with_methods: Some(MethodSet::from(&[MethodKind::Password][..])),
                partial_success: true,
            }
        } else {
            Auth::reject()
        })
    }
}

#[tokio::test]
async fn a_key_taken_as_a_first_factor_is_accepted() {
    let ours = generate_key("filebridge-p-test").unwrap();
    let host_key = generate_key("host").unwrap().key;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let config = Arc::new(server::Config {
        keys: vec![PrivateKey::clone(&host_key)],
        auth_rejection_time: std::time::Duration::from_millis(1),
        auth_rejection_time_initial: Some(std::time::Duration::ZERO),
        ..Default::default()
    });
    let first_factor = ours.key.public_key().clone();
    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            let handler = TwoFactor {
                first_factor: first_factor.clone(),
            };
            if let Ok(session) = server::run_stream(config.clone(), stream, handler).await {
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
    let target = SshTarget {
        host: "127.0.0.1".into(),
        port,
        user: "alice".into(),
        key_file: None,
        known_hosts_file: Some(known),
        key: None,
    };
    let connector = SshConnector::new();
    assert!(connector.key_accepted(&target, ours.key).await.unwrap());
    let other = generate_key("someone-else").unwrap();
    assert!(!connector.key_accepted(&target, other.key).await.unwrap());
}
