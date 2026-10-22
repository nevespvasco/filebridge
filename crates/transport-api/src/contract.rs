//! The `Transport` contract as a test suite. Every adapter runs [`check`] against a destination
//! it can write to, so the fake used by `core`'s tests and rclone keep the same promises.

use std::path::Path;

use sha2::{Digest as _, Sha256};

use crate::{
    Destination, ErrorKind, HashAlgo, ItemOutcome, MoveRequest, ProgressEvent, ProgressSink,
    RemotePath, TransferItem, TransferJob, TransferOptions, Transport, TransportCaps,
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
        source_root: src.clone(),
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

        // Many at once: the same answers, in order, across folders (the root among them), for
        // names with characters that are special to globs, and for a path that isn't there.
        let extra: [(&str, &[u8], &str); 2] = [
            ("x.txt", b"brackets", "Fotos/x [1] {a,b} #.txt"),
            ("y.txt", b"stars", "deep/er/y*?\\.txt"),
        ];
        let extra = if cfg!(windows) {
            &extra[..1]
        } else {
            &extra[..]
        };
        let job = TransferJob {
            id: "contract-hash-many".into(),
            source_root: src.clone(),
            items: extra
                .iter()
                .map(|(name, data, to)| TransferItem {
                    source: name.into(),
                    dest: RemotePath::new(*to).unwrap(),
                    size: data.len() as u64,
                    expected_hash: None,
                })
                .collect(),
            options: TransferOptions::default(),
        };
        for (name, data, _) in extra {
            tokio::fs::write(src.join(name), data).await.unwrap();
        }
        let sent = transport
            .transfer(dest, &job, ProgressSink::noop())
            .await
            .unwrap();
        assert_eq!(sent.failed(), 0, "{:?}", sent.items);
        let mut want: Vec<(&str, Option<&[u8]>)> = Vec::new();
        for (i, (_, data, to)) in files.iter().chain(extra).enumerate() {
            want.push((*to, Some(*data)));
            if i == 1 {
                want.push(("not/there.txt", None));
                want.push(("deep/er/missing.bin", None));
            }
        }
        let paths: Vec<_> = want
            .iter()
            .map(|(p, _)| RemotePath::new(*p).unwrap())
            .collect();
        let many = transport.hash_many(dest, &paths, HashAlgo::Sha256).await;
        assert_eq!(many.len(), paths.len(), "one answer per path");
        for ((path, data), got) in want.iter().zip(&many) {
            match (data, got) {
                (Some(data), Ok(d)) => {
                    assert_eq!(d.bytes, Sha256::digest(data).to_vec(), "{path}")
                }
                (None, Err(e)) => assert_eq!(e.kind, ErrorKind::NotFound, "{path}: {e:?}"),
                (_, got) => panic!("{path}: {got:?}"),
            }
        }
        assert!(
            transport
                .hash_many(dest, &[], HashAlgo::Sha256)
                .await
                .is_empty()
        );

        // Sent again into folders that already hold files: a changed file is replaced, an
        // unchanged one stays, new ones arrive next to them.
        let again: [(&str, &[u8], &str); 4] = [
            ("a.txt", b"alpha, changed", "a.txt"),
            ("a2.txt", b"new at the root", "a2.txt"),
            ("sub/b.bin", &[0, 1, 2, 255], "deep/er/b.bin"),
            ("c.txt", b"new next to b", "deep/er/c.txt"),
        ];
        for (name, data, _) in again {
            tokio::fs::write(src.join(name), data).await.unwrap();
        }
        let job = TransferJob {
            id: "contract-again".into(),
            source_root: src.clone(),
            items: again
                .iter()
                .map(|(name, data, to)| TransferItem {
                    source: name.into(),
                    dest: RemotePath::new(*to).unwrap(),
                    size: data.len() as u64,
                    expected_hash: None,
                })
                .collect(),
            options: TransferOptions::default(),
        };
        let sent = transport
            .transfer(dest, &job, ProgressSink::noop())
            .await
            .unwrap();
        assert_eq!(sent.failed(), 0, "{:?}", sent.items);
        let paths: Vec<_> = again
            .iter()
            .map(|(_, _, to)| RemotePath::new(*to).unwrap())
            .collect();
        let hashes = transport.hash_many(dest, &paths, HashAlgo::Sha256).await;
        for ((_, data, to), got) in again.iter().zip(hashes) {
            assert_eq!(got.unwrap().bytes, Sha256::digest(data).to_vec(), "{to}");
        }
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
/// spelling, a name already taken (exactly, in another normalisation or in another case) is
/// never overwritten, a move cut short between its hops is finished, one already done is
/// recognised, a leftover intermediate name stops the move, and a missing source is `NotFound`.
pub async fn check_move(transport: &dyn Transport, dest: &Destination, scratch: &Path) {
    assert!(transport.capabilities().contains(TransportCaps::MOVE));
    let src = scratch.join("contract-move-src");
    let seed: [(&str, &str); 8] = [
        ("a.txt", "mv/a.txt"),
        ("b.txt", "mv/dir/b.txt"),
        ("t.txt", "mv/taken.txt"),
        ("n.txt", "mv/Vera\u{303}o.txt"),
        ("e.txt", "mv/Cafe\u{301}.txt"),
        ("c.txt", "mv/c.txt"),
        ("d.txt", "mv2/d.txt"),
        ("x.txt", "mv2/.fb-move-d.txt"),
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
    // The intermediate name a caller would pick: deterministic, in the source's folder.
    let req = |from: &str, to: &str, is_dir: bool| {
        let from = p(from);
        let via = match from.parent() {
            Some(dir) => format!("{dir}/.fb-move-{}", from.file_name()),
            None => format!(".fb-move-{}", from.file_name()),
        };
        MoveRequest {
            to: p(to),
            via: p(&via),
            from,
            is_dir,
            size: None,
            mtime: None,
        }
    };
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

    for (from, to, is_dir) in [
        ("mv/a.txt", "mv/a2.txt", false),
        ("mv/dir", "mv/dir2", true),
        ("mv/Vera\u{303}o.txt", "mv/Ver\u{e3}o.txt", false),
    ] {
        transport
            .move_path(dest, &req(from, to, is_dir))
            .await
            .unwrap_or_else(|e| panic!("{from} -> {to}: {e}"));
    }
    let listed = names("mv").await;
    for want in ["a2.txt", "dir2", "taken.txt", "Ver\u{e3}o.txt"] {
        assert!(listed.contains(&want.to_owned()), "{want} in {listed:?}");
    }
    for gone in ["a.txt", "dir", "Vera\u{303}o.txt"] {
        assert!(!listed.contains(&gone.to_owned()), "{gone} in {listed:?}");
    }
    assert_eq!(names("mv/dir2").await, ["b.txt"]);

    // Taken exactly, in another case, or in another normalisation: refused, nothing moves.
    for to in ["mv/taken.txt", "mv/TAKEN.txt", "mv/Caf\u{e9}.txt"] {
        let refused = transport
            .move_path(dest, &req("mv/a2.txt", to, false))
            .await;
        assert!(
            refused.is_err(),
            "{to}: an existing name is never overwritten"
        );
    }
    let listed = names("mv").await;
    for kept in ["a2.txt", "taken.txt", "Cafe\u{301}.txt"] {
        assert!(listed.contains(&kept.to_owned()), "{kept} in {listed:?}");
    }
    if let Ok(d) = transport
        .hash(dest, &p("mv/taken.txt"), HashAlgo::Sha256)
        .await
    {
        assert_eq!(
            d.bytes,
            Sha256::digest(b"t.txt").to_vec(),
            "taken.txt untouched"
        );
    }

    // Cut short between the hops: asking again finishes it.
    let cut = req("mv/c.txt", "mv/c2.txt", false);
    transport
        .move_unchecked(dest, &cut.from, &cut.via, false)
        .await
        .unwrap();
    transport.move_path(dest, &cut).await.unwrap();
    let listed = names("mv").await;
    assert!(listed.contains(&"c2.txt".to_owned()), "{listed:?}");
    // Asked again once done: done, if `to` is the entry that was moved.
    transport.move_path(dest, &cut).await.unwrap();
    let other = MoveRequest {
        size: Some(999),
        ..cut.clone()
    };
    let err = transport.move_path(dest, &other).await.unwrap_err();
    assert_eq!(err.kind, ErrorKind::NotFound, "another file at c2.txt");
    assert!(
        names("mv").await.iter().all(|n| !n.starts_with(".fb-")),
        "no intermediate name left"
    );

    // A leftover intermediate name stops the move: both stay as they are.
    let err = transport
        .move_path(dest, &req("mv2/d.txt", "mv2/d2.txt", false))
        .await
        .unwrap_err();
    assert_ne!(err.kind, ErrorKind::NotFound, "{err:?}");
    assert_eq!(names("mv2").await, [".fb-move-d.txt", "d.txt"]);

    let missing = transport
        .move_path(dest, &req("mv/nope.txt", "mv/nope2.txt", false))
        .await
        .unwrap_err();
    assert_eq!(missing.kind, ErrorKind::NotFound);
}
