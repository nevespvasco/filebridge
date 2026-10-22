//! Check: per-file `Transport::hash` and `Transport::hash_many` give the same answer for every
//! file a `verify_bench` run sent (BENCH_ROOT of that run, same env), plus a file and a folder
//! that aren't there, and how long each takes. Change or delete a few files on the server first
//! to compare mismatches and `NotFound` too.

use std::sync::Arc;
use std::time::Instant;

use filebridge_secrets::{MemoryStore, SecretKind, SecretStore, StoreResolver, reference};
use filebridge_transport_api::{Backend, Destination, HashAlgo, RemotePath, Transport};
use filebridge_transport_rclone::{Rcd, RcloneTransport, Strategy};

#[tokio::main]
async fn main() {
    let env = |k: &str| std::env::var(k).unwrap_or_else(|_| panic!("{k}"));
    let n: usize = env("BENCH_FILES").parse().unwrap();
    let secrets = Arc::new(MemoryStore::default());
    let pass_ref = reference("cmp", SecretKind::AppPassword);
    secrets.set(&pass_ref, &env("BENCH_PASS")).unwrap();
    let rclone = filebridge_host::find_rclone(None).unwrap();
    let rcd = Arc::new(Rcd::start(&rclone).await.unwrap());
    let t = RcloneTransport::new(rcd, Strategy::PerFile)
        .with_secrets(Arc::new(StoreResolver(secrets)));
    let dest = Destination {
        transport: "rclone".into(),
        backend: Backend::Webdav {
            url: env("BENCH_URL"),
            user: env("BENCH_USER"),
            password: Some(pass_ref),
            vendor: Some("nextcloud".into()),
        },
        root: env("BENCH_ROOT"),
    };
    let mut paths = Vec::new();
    for i in 0..n {
        let sub = match i % 60 {
            0 => "a/",
            1 => "a/b/",
            2 => "c/",
            _ => "",
        };
        paths.push(RemotePath::new(format!("bench/{sub}file-{i:05} [x]{{y}}.txt")).unwrap());
    }
    paths.push(RemotePath::new("bench/never-sent.txt").unwrap());
    paths.push(RemotePath::new("bench/no-folder/x.txt").unwrap());

    let t0 = Instant::now();
    let many = t.hash_many(&dest, &paths, HashAlgo::Sha256).await;
    let many_secs = t0.elapsed().as_secs_f64();
    let t0 = Instant::now();
    let mut one = Vec::new();
    for p in &paths {
        one.push(t.hash(&dest, p, HashAlgo::Sha256).await);
    }
    let one_secs = t0.elapsed().as_secs_f64();

    let key = |r: &filebridge_transport_api::Result<filebridge_transport_api::Digest>| match r {
        Ok(d) => format!("ok {}", d.hex()),
        Err(e) => format!("err {:?}", e.kind),
    };
    let mut same = 0;
    for (p, (a, b)) in paths.iter().zip(many.iter().zip(&one)) {
        if key(a) == key(b) {
            same += 1;
        } else {
            println!("DIFFERENT {p}: many={} one={}", key(a), key(b));
        }
    }
    let ok = many.iter().filter(|r| r.is_ok()).count();
    let errs: Vec<_> = many
        .iter()
        .filter_map(|r| r.as_ref().err().map(|e| e.kind))
        .collect();
    println!(
        "paths={} same={same} ok={ok} errors={errs:?} hash_many={many_secs:.1}s hash_per_file={one_secs:.1}s",
        paths.len()
    );
}
