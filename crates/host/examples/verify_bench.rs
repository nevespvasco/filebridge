//! Benchmark: sends BENCH_FILES small files (most in one folder, a few in subfolders, names
//! with glob characters) to a Nextcloud over WebDAV with verify on, through the real engine and
//! rclone, and times the transfer and verify stages. The password stays in memory, never in the
//! keyring.
//!
//! Env: BENCH_URL (`…/remote.php/dav/files/<user>/`), BENCH_USER, BENCH_PASS, BENCH_ROOT (a
//! folder under the user's files), BENCH_FILES. BENCH_PASS should be an app password, as the app
//! stores: Nextcloud checks an account password with a slow hash on every request, which makes
//! each request about 4× slower. Use a throwaway server and account, e.g.
//! `docker run -p 127.0.0.1:8099:80 -e SQLITE_DATABASE=nc -e NEXTCLOUD_ADMIN_USER=… \
//! -e NEXTCLOUD_ADMIN_PASSWORD=… nextcloud:31-apache`. See crates/transport-rclone/README.md.

use std::sync::Arc;
use std::time::{Duration, Instant};

use filebridge_core::backoff::Backoff;
use filebridge_core::{
    Engine, EngineOptions, Event, JobState, LocalClock, Profile, Registry, SendOptions, Store,
    TargetSpec,
};
use filebridge_secrets::{MemoryStore, SecretKind, SecretStore, StoreResolver, reference};
use filebridge_transport_api::{Backend, Destination};
use filebridge_transport_rclone::{Rcd, RcloneTransport, Strategy};

#[tokio::main]
async fn main() {
    let env = |k: &str| std::env::var(k).unwrap_or_else(|_| panic!("{k}"));
    let n: usize = env("BENCH_FILES").parse().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("src/bench");
    std::fs::create_dir_all(src.join("a/b")).unwrap();
    std::fs::create_dir_all(src.join("c")).unwrap();
    for i in 0..n {
        let sub = match i % 60 {
            0 => "a/",
            1 => "a/b/",
            2 => "c/",
            _ => "",
        };
        std::fs::write(
            src.join(format!("{sub}file-{i:05} [x]{{y}}.txt")),
            format!("content of file {i}\n").repeat(1 + i % 7),
        )
        .unwrap();
    }

    let secrets = Arc::new(MemoryStore::default());
    let pass_ref = reference("bench", SecretKind::AppPassword);
    secrets.set(&pass_ref, &env("BENCH_PASS")).unwrap();
    let rclone = filebridge_host::find_rclone(None).unwrap();
    let rcd = Arc::new(Rcd::start(&rclone).await.unwrap());
    let mut registry = Registry::new();
    registry.add_transport(Arc::new(
        RcloneTransport::new(rcd.clone(), Strategy::PerFile)
            .with_secrets(Arc::new(StoreResolver(secrets.clone()))),
    ));
    registry.set_secrets(secrets.clone());
    let store = Store::open_in_memory().unwrap();
    let engine = Engine::new(
        store,
        registry,
        EngineOptions {
            backoff: Backoff::Fixed(Duration::from_millis(500)),
            inhibit_sleep: false,
            local_clock: LocalClock::default(),
            ..EngineOptions::default()
        },
    );
    let profile = engine
        .save_profile(Profile {
            id: String::new(),
            name: "bench".into(),
            destination: Destination {
                transport: RcloneTransport::ID.into(),
                backend: Backend::Webdav {
                    url: env("BENCH_URL"),
                    user: env("BENCH_USER"),
                    password: None,
                    vendor: Some("nextcloud".into()),
                },
                root: env("BENCH_ROOT"),
            },
            integration: None,
            integration_settings: serde_json::json!({}),
            target: TargetSpec::default(),
        })
        .await
        .unwrap();
    engine
        .set_profile_password(&profile.id, Some(env("BENCH_PASS")))
        .await
        .unwrap();

    let job = engine
        .create_job(
            vec![src.clone()],
            &profile.id,
            SendOptions {
                verify: true,
                ..SendOptions::default()
            },
        )
        .await
        .unwrap();
    let summary = engine.analyse(&job).await.unwrap();
    assert_eq!(summary.state, JobState::Ready, "{summary:?}");

    let mut events = engine.subscribe();
    let watch = tokio::spawn(async move {
        let (mut t0, mut verify_start, mut verify_secs) = (Instant::now(), None, 0.0);
        let (mut send_start, mut send_secs) = (None, 0.0);
        while let Ok(ev) = events.recv().await {
            if let Event::JobStateChanged { from, to, .. } = ev {
                eprintln!("{:>8.1}s {from:?} -> {to:?}", t0.elapsed().as_secs_f64());
                if to == JobState::Transferring && verify_start.is_none() {
                    t0 = Instant::now();
                }
                if to == JobState::Transferring {
                    send_start = Some(Instant::now());
                }
                if from == JobState::Transferring
                    && let Some(s) = send_start.take()
                {
                    send_secs += s.elapsed().as_secs_f64();
                }
                if to == JobState::Verifying {
                    verify_start = Some(Instant::now());
                }
                if from == JobState::Verifying
                    && let Some(s) = verify_start.take()
                {
                    verify_secs += s.elapsed().as_secs_f64();
                }
                if matches!(to, JobState::Completed | JobState::Failed) {
                    break;
                }
            }
        }
        (send_secs, verify_secs)
    });
    engine.start(&job).await.unwrap();
    let state = engine.wait(&job).await.unwrap();
    let (send_secs, verify_secs) = watch.await.unwrap();
    let s = engine.job(&job).await.unwrap();
    println!(
        "state={state:?} files={} done={} verified_ok={} mismatch={} send_secs={send_secs:.1} \
         sent_per_sec={:.1} verify_secs={verify_secs:.1} verified_per_sec={:.1} error={:?}",
        s.counts.files,
        s.counts.done,
        s.counts.verified_ok,
        s.counts.mismatch,
        s.counts.done as f64 / send_secs,
        s.counts.verified_ok as f64 / verify_secs,
        s.error,
    );
}
