//! SMB through rclone's `smb` backend, against a real share with the password kept in the
//! keyring (ADR 0022): the shared contract holds (hashes by download: SMB has none), a wrong
//! password is an `Auth` error, and no error repeats the password.
//!
//! rclone has no `serve smb`, so this runs only against a server given by
//! `FILEBRIDGE_TEST_SMB_HOST`, `_SHARE`, `_USER` and `_PASSWORD` (and optionally `_PORT`,
//! `_DOMAIN`); the share must be writable and is written under a fresh folder. A local Samba in
//! Docker: `crates/transport-rclone/README.md`. Skipped otherwise, and when rclone isn't
//! installed.

use std::path::PathBuf;
use std::sync::Arc;

use filebridge_secrets::{MemoryStore, SecretKind, SecretStore, StoreResolver, reference};
use filebridge_transport_api::{Backend, CheckStatus, Destination, ErrorKind, Transport};
use filebridge_transport_rclone::{Rcd, RcloneTransport};

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

fn var(name: &str) -> Option<String> {
    std::env::var(format!("FILEBRIDGE_TEST_SMB_{name}"))
        .ok()
        .filter(|v| !v.is_empty())
}

#[tokio::test]
async fn smb_with_a_password_from_the_keyring() {
    let (Some(host), Some(share), Some(user), Some(password)) =
        (var("HOST"), var("SHARE"), var("USER"), var("PASSWORD"))
    else {
        eprintln!("skipped: FILEBRIDGE_TEST_SMB_HOST/_SHARE/_USER/_PASSWORD not set");
        return;
    };
    let Some(binary) = rclone() else {
        eprintln!("skipped: rclone not found");
        return;
    };
    let store = Arc::new(MemoryStore::default());
    let pass_ref = reference("p-smb", SecretKind::AppPassword);
    store.set(&pass_ref, &password).unwrap();
    let rcd = Arc::new(Rcd::start(&binary).await.unwrap());
    let transport = RcloneTransport::new(rcd).with_secrets(Arc::new(StoreResolver(store.clone())));
    let folder = format!("filebridge-test-{}", std::process::id());
    let dest = Destination {
        transport: "rclone".into(),
        backend: Backend::Smb {
            host,
            port: var("PORT").map(|p| p.parse().expect("FILEBRIDGE_TEST_SMB_PORT")),
            user: Some(user),
            domain: var("DOMAIN"),
            share,
            password: Some(pass_ref.clone()),
        },
        root: folder,
    };

    // The folder isn't there yet: reachable, created by the first send.
    let report = transport.preflight(&dest).await.unwrap();
    let connect = report.checks.iter().find(|c| c.id == "connect").unwrap();
    assert_eq!(connect.status, CheckStatus::Warn, "{connect:?}");

    let dir = tempfile::tempdir().unwrap();
    filebridge_transport_api::contract::check(&transport, &dest, dir.path()).await;
    filebridge_transport_api::contract::check_move(&transport, &dest, dir.path()).await;
    let report = transport.preflight(&dest).await.unwrap();
    let connect = report.checks.iter().find(|c| c.id == "connect").unwrap();
    assert_eq!(connect.status, CheckStatus::Ok, "{connect:?}");
    assert!(report.free_bytes.is_some(), "SMB reports its free space");

    // A share that isn't there fails the test; it isn't a folder a send creates.
    let mut no_share = dest.clone();
    if let Backend::Smb { share, .. } = &mut no_share.backend {
        *share = "no-such-share".into();
    }
    let report = transport.preflight(&no_share).await.unwrap();
    let connect = report.checks.iter().find(|c| c.id == "connect").unwrap();
    assert_eq!(connect.status, CheckStatus::Fail, "{connect:?}");

    store.set(&pass_ref, "wrong-smb-password").unwrap();
    let err = transport.list(&dest, None, false).await.unwrap_err();
    assert_eq!(err.kind, ErrorKind::Auth, "{err:?}");
    for secret in [password.as_str(), "wrong-smb-password"] {
        assert!(!err.message.contains(secret), "{}", err.message);
    }
}
