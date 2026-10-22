//! The `Transport` contract as a test suite. Every adapter runs [`check`] against a destination
//! it can write to, so the fake used by `core`'s tests and rclone keep the same promises.

use std::path::Path;

use sha2::{Digest as _, Sha256};

use crate::{
    Destination, ErrorKind, HashAlgo, ItemOutcome, ProgressEvent, ProgressSink, RemotePath,
    TransferItem, TransferJob, TransferOptions, Transport, TransportCaps,
};

/// Sends a small tree from a source under `scratch` to `dest` and checks what the contract
/// promises: corrected names and new parent directories, one outcome per item in order, a
/// missing source file as `NotFound`, `ItemDone` per file, and hashes (`NotFound` for a path
/// that isn't there, `Unsupported` for a kind the destination lacks). Panics on a broken promise.
pub async fn check(transport: &dyn Transport, dest: &Destination, scratch: &Path) {
    let src = scratch.join("contract-src");
    let files: [(&str, &[u8], &str); 3] = [
        ("a.txt", b"alpha", "a.txt"),
        ("sub/b.bin", &[0, 1, 2, 255], "deep/er/b.bin"),
        // NFD source name, NFC destination name: the rename happens in transit.
        (
            "Vera\u{303}o.txt",
            "verão".as_bytes(),
            "Fotos/Ver\u{e3}o.txt",
        ),
    ];
    for (name, data, _) in files {
        let path = src.join(name);
        tokio::fs::create_dir_all(path.parent().unwrap())
            .await
            .unwrap();
        tokio::fs::write(&path, data).await.unwrap();
    }
    let mut items: Vec<_> = files
        .iter()
        .map(|(name, data, to)| TransferItem {
            source: name.into(),
            dest: RemotePath::new(*to).unwrap(),
            size: data.len() as u64,
            expected_hash: None,
        })
        .collect();
    items.push(TransferItem {
        source: "missing.txt".into(),
        dest: RemotePath::new("missing.txt").unwrap(),
        size: 1,
        expected_hash: None,
    });
    let job = TransferJob {
        id: "contract".into(),
        source_root: src,
        items,
        options: TransferOptions::default(),
    };

    let (sink, mut rx) = ProgressSink::channel();
    let result = transport.transfer(dest, &job, sink).await.unwrap();
    assert_eq!(result.items.len(), 4, "one outcome per item");
    for (i, outcome) in result.items[..3].iter().enumerate() {
        assert!(
            matches!(outcome, ItemOutcome::Done),
            "item {i}: {outcome:?}"
        );
    }
    match &result.items[3] {
        ItemOutcome::Failed(e) => assert_eq!(e.kind, ErrorKind::NotFound, "{e:?}"),
        other => panic!("a missing source file can't be Done: {other:?}"),
    }
    let mut done = Vec::new();
    while let Ok(event) = rx.try_recv() {
        if let ProgressEvent::ItemDone { index, ok: true } = event {
            done.push(index);
        }
    }
    done.sort_unstable();
    assert_eq!(done, [0, 1, 2], "ItemDone for every file that arrived");

    let caps = transport.capabilities();
    if caps.contains(TransportCaps::HASH_SHA256) {
        for (_, data, to) in files {
            let path = RemotePath::new(to).unwrap();
            let digest = transport.hash(dest, &path, HashAlgo::Sha256).await.unwrap();
            assert_eq!(digest.bytes, Sha256::digest(data).to_vec(), "{to}");
        }
        let gone = RemotePath::new("not/there.txt").unwrap();
        let err = transport
            .hash(dest, &gone, HashAlgo::Sha256)
            .await
            .unwrap_err();
        assert_eq!(err.kind, ErrorKind::NotFound, "{err:?}");
    }
    if !caps.contains(TransportCaps::HASH_XXH) {
        let path = RemotePath::new("a.txt").unwrap();
        let err = transport
            .hash(dest, &path, HashAlgo::Xxh3_128)
            .await
            .unwrap_err();
        assert_eq!(err.kind, ErrorKind::Unsupported, "{err:?}");
    }
}
