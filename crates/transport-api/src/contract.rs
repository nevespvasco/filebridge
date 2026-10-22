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

    // Space, when reported, adds up: free never beyond total, used + free about total (some
    // filesystems keep reserved blocks, hence the 2 % slack).
    let report = transport.preflight(dest).await.unwrap();
    if let (Some(free), Some(total)) = (report.free_bytes, report.total_bytes) {
        assert!(free <= total, "free {free} > total {total}");
        if let Some(used) = report.used_bytes {
            assert!(
                used + free <= total + total / 50,
                "used {used} + free {free} > total {total}"
            );
        }
    }

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
        match event {
            ProgressEvent::ItemDone { index, ok: true } => done.push(index),
            // A transport may not know what is in flight (an empty list), but what it names
            // must be one of the job's items, under its destination name.
            ProgressEvent::Stats(s) => {
                for f in &s.transferring {
                    assert!(
                        job.items.iter().any(|i| i.dest.as_str() == f.dest),
                        "in flight but not in the job: {:?}",
                        f.dest
                    );
                }
            }
            _ => {}
        }
    }
    done.sort_unstable();
    assert_eq!(done, [0, 1, 2], "ItemDone for every file that arrived");

    // Listing, for adapters that say they can: what was sent is there, folders only on request,
    // and a folder that isn't there is NotFound.
    if transport.capabilities().contains(TransportCaps::LIST) {
        let top = transport.list(dest, None, false).await.unwrap();
        for name in ["a.txt", "deep", "Fotos"] {
            assert!(
                top.iter().any(|e| e.name == name),
                "{name} at the root: {top:?}"
            );
        }
        let dirs = transport.list(dest, None, true).await.unwrap();
        assert!(dirs.iter().all(|e| e.is_dir), "folders only: {dirs:?}");
        assert!(dirs.iter().any(|e| e.name == "deep"));
        let inside = transport
            .list(dest, Some(&RemotePath::new("deep/er").unwrap()), false)
            .await
            .unwrap();
        assert_eq!(
            inside.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(),
            ["b.bin"]
        );
        let missing = transport
            .list(
                dest,
                Some(&RemotePath::new("no-such-folder").unwrap()),
                false,
            )
            .await
            .unwrap_err();
        assert_eq!(missing.kind, ErrorKind::NotFound, "{missing:?}");
    }

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

/// The promise of [`Transport::move_path`], for adapters with [`TransportCaps::MOVE`]: a file
/// and a folder (with what is inside) are renamed in place, an NFD name becomes its NFC
/// spelling, an existing name is never overwritten, and a missing source is `NotFound`.
pub async fn check_move(transport: &dyn Transport, dest: &Destination, scratch: &Path) {
    assert!(transport.capabilities().contains(TransportCaps::MOVE));
    let src = scratch.join("contract-move-src");
    let seed: [(&str, &str); 4] = [
        ("a.txt", "mv/a.txt"),
        ("b.txt", "mv/dir/b.txt"),
        ("t.txt", "mv/taken.txt"),
        ("n.txt", "mv/Vera\u{303}o.txt"),
    ];
    let mut items = Vec::new();
    for (name, to) in seed {
        tokio::fs::create_dir_all(&src).await.unwrap();
        tokio::fs::write(src.join(name), name.as_bytes())
            .await
            .unwrap();
        items.push(TransferItem {
            source: name.into(),
            dest: RemotePath::new(to).unwrap(),
            size: name.len() as u64,
            expected_hash: None,
        });
    }
    let job = TransferJob {
        id: "contract-move".into(),
        source_root: src,
        items,
        options: TransferOptions::default(),
    };
    transport
        .transfer(dest, &job, ProgressSink::noop())
        .await
        .unwrap();
    let p = |s: &str| RemotePath::new(s).unwrap();
    let names = |dir: &'static str| async move {
        let mut v: Vec<String> = transport
            .list(dest, Some(&RemotePath::new(dir).unwrap()), false)
            .await
            .unwrap()
            .into_iter()
            .map(|e| e.name)
            .collect();
        v.sort();
        v
    };

    transport
        .move_path(dest, &p("mv/a.txt"), &p("mv/a2.txt"))
        .await
        .unwrap();
    transport
        .move_path(dest, &p("mv/dir"), &p("mv/dir2"))
        .await
        .unwrap();
    transport
        .move_path(dest, &p("mv/Vera\u{303}o.txt"), &p("mv/Ver\u{e3}o.txt"))
        .await
        .unwrap();
    let listed = names("mv").await;
    for want in ["a2.txt", "dir2", "taken.txt", "Ver\u{e3}o.txt"] {
        assert!(listed.contains(&want.to_owned()), "{want} in {listed:?}");
    }
    for gone in ["a.txt", "dir", "Vera\u{303}o.txt"] {
        assert!(!listed.contains(&gone.to_owned()), "{gone} in {listed:?}");
    }
    assert!(
        listed.iter().all(|n| !n.starts_with(".fb-")),
        "no intermediate name left: {listed:?}"
    );
    assert_eq!(names("mv/dir2").await, ["b.txt"]);

    let refused = transport
        .move_path(dest, &p("mv/a2.txt"), &p("mv/taken.txt"))
        .await;
    assert!(refused.is_err(), "an existing name is never overwritten");
    let listed = names("mv").await;
    assert!(listed.contains(&"a2.txt".to_owned()) && listed.contains(&"taken.txt".to_owned()));
    let digest = transport
        .hash(dest, &p("mv/taken.txt"), HashAlgo::Sha256)
        .await;
    if let Ok(d) = digest {
        assert_eq!(
            d.bytes,
            Sha256::digest(b"t.txt").to_vec(),
            "taken.txt untouched"
        );
    }

    let missing = transport
        .move_path(dest, &p("mv/nope.txt"), &p("mv/nope2.txt"))
        .await
        .unwrap_err();
    assert_eq!(missing.kind, ErrorKind::NotFound);
}
