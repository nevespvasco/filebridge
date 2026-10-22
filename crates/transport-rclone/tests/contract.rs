//! The shared `Transport` contract against a real `rclone rcd` on the local backend. Skipped when
//! rclone isn't installed (`RCLONE`, then `PATH`, then `~/.local/bin`).

use std::path::PathBuf;
use std::sync::Arc;

use filebridge_transport_api::{Backend, Destination};
use filebridge_transport_rclone::{Rcd, RcloneTransport, Strategy};

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

#[tokio::test]
async fn per_file_keeps_the_transport_contract() {
    let Some(binary) = rclone() else {
        eprintln!("skipped: rclone not found");
        return;
    };
    let rcd = Arc::new(Rcd::start(&binary).await.unwrap());
    let transport = RcloneTransport::new(rcd, Strategy::PerFile);
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("dest");
    std::fs::create_dir(&root).unwrap();
    let dest = Destination {
        transport: "rclone".into(),
        backend: Backend::Local,
        root: root.to_string_lossy().into_owned(),
    };
    filebridge_transport_api::contract::check(&transport, &dest, dir.path()).await;
}
