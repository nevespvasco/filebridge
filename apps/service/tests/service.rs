//! Runs the real `filebridge-service` binary with real rclone (skipped when rclone is missing)
//! and drives it over the socket like the desktop app does.
#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use filebridge_core::{Event, JobState, Profile, SendOptions, TargetSpec};
use filebridge_ipc::{Client, Endpoint};
use filebridge_transport_api::{Backend, Destination};

struct Env {
    dir: tempfile::TempDir,
    rclone: PathBuf,
}

impl Env {
    fn new() -> Option<Self> {
        let rclone = filebridge_host::find_rclone(None).ok()?;
        Some(Self {
            dir: tempfile::tempdir().unwrap(),
            rclone,
        })
    }

    fn socket(&self) -> PathBuf {
        self.dir.path().join("run/service.sock")
    }

    fn spawn(&self) -> Child {
        Command::new(env!("CARGO_BIN_EXE_filebridge-service"))
            .arg("--data-dir")
            .arg(self.dir.path().join("data"))
            .arg("--socket")
            .arg(self.socket())
            .arg("--rclone")
            .arg(&self.rclone)
            .arg("run")
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap()
    }

    async fn connect(&self) -> Client {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            if let Ok(c) = Client::connect(&Endpoint::Path(self.socket())).await {
                return c;
            }
            assert!(Instant::now() < deadline, "service did not come up");
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }
}

fn signal(child: &Child, sig: &str) {
    let ok = Command::new("kill")
        .args([sig, &child.id().to_string()])
        .status()
        .unwrap()
        .success();
    assert!(ok);
}

fn fixture(root: &Path, files: usize, size: usize) -> PathBuf {
    let src = root.join("src/Fotos");
    std::fs::create_dir_all(src.join("Vera\u{303}o")).unwrap();
    for i in 0..files {
        std::fs::write(
            src.join(format!("Vera\u{303}o/f{i:03}.bin")),
            vec![i as u8; size],
        )
        .unwrap();
    }
    src
}

async fn local_profile(client: &Client, dest: &Path) -> Profile {
    std::fs::create_dir_all(dest).unwrap();
    client
        .save_profile(Profile {
            id: String::new(),
            name: "local".into(),
            destination: Destination {
                transport: "rclone".into(),
                backend: Backend::Local,
                root: dest.to_string_lossy().into_owned(),
            },
            integration: None,
            integration_settings: serde_json::Value::Null,
            target: TargetSpec::default(),
        })
        .await
        .unwrap()
}

async fn wait_finished(client: &Client, job: &str, timeout: Duration) -> JobState {
    let deadline = Instant::now() + timeout;
    loop {
        let j = client.job(job.to_owned()).await.unwrap();
        if j.state.is_terminal() {
            return j.state;
        }
        assert!(Instant::now() < deadline, "job still {:?}", j.state);
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

#[tokio::test]
async fn serves_a_send_refuses_a_second_instance_and_stops_cleanly() {
    let Some(env) = Env::new() else {
        eprintln!("skipped: rclone not found");
        return;
    };
    let mut service = env.spawn();
    let client = env.connect().await;
    assert_eq!(client.ping().await.unwrap().protocol, 1);

    // A second instance on the same data dir must refuse to start.
    let second = env.spawn().wait_with_output().unwrap();
    assert!(!second.status.success());
    let stderr = String::from_utf8_lossy(&second.stderr);
    assert!(stderr.contains("another filebridge engine"), "{stderr}");

    let src = fixture(env.dir.path(), 5, 1000);
    let dest = env.dir.path().join("dest");
    let profile = local_profile(&client, &dest).await;
    let mut events = client.events().await.unwrap();
    let job = client
        .create_job(
            vec![src.to_string_lossy().into_owned()],
            profile.id,
            SendOptions::default(),
        )
        .await
        .unwrap();
    assert_eq!(
        client.analyse(job.clone()).await.unwrap().state,
        JobState::Ready
    );
    client.start(job.clone()).await.unwrap();
    assert_eq!(
        wait_finished(&client, &job, Duration::from_secs(60)).await,
        JobState::Completed
    );
    assert!(
        dest.join("Fotos/Verão/f004.bin").exists(),
        "NFC name at the destination"
    );
    let mut saw_finish = false;
    while let Ok(e) = events.try_recv() {
        saw_finish |= matches!(e, Event::JobFinished { .. });
    }
    assert!(saw_finish, "events reach subscribed clients");

    signal(&service, "-TERM");
    let status = service.wait().unwrap();
    assert!(status.success(), "clean exit on SIGTERM: {status}");
    assert!(!env.socket().exists(), "socket removed on shutdown");
}

#[tokio::test]
async fn a_killed_service_resumes_its_jobs_on_restart() {
    let Some(env) = Env::new() else {
        eprintln!("skipped: rclone not found");
        return;
    };
    let mut service = env.spawn();
    let client = env.connect().await;
    let src = fixture(env.dir.path(), 20, 100_000);
    let dest = env.dir.path().join("dest");
    let profile = local_profile(&client, &dest).await;
    let job = client
        .create_job(
            vec![src.to_string_lossy().into_owned()],
            profile.id,
            SendOptions {
                bwlimit: Some("400k".into()),
                concurrency: 2,
                ..SendOptions::default()
            },
        )
        .await
        .unwrap();
    client.analyse(job.clone()).await.unwrap();
    client.start(job.clone()).await.unwrap();
    tokio::time::sleep(Duration::from_secs(2)).await;
    signal(&service, "-KILL");
    service.wait().unwrap();
    drop(client);

    let mut service = env.spawn();
    let client = env.connect().await;
    let state = wait_finished(&client, &job, Duration::from_secs(60)).await;
    assert_eq!(state, JobState::Completed);
    assert_eq!(client.job(job).await.unwrap().counts.done, 20);
    for i in 0..20 {
        let f = dest.join(format!("Fotos/Verão/f{i:03}.bin"));
        assert_eq!(
            std::fs::read(&f).unwrap(),
            vec![i as u8; 100_000],
            "{}",
            f.display()
        );
    }
    signal(&service, "-TERM");
    service.wait().unwrap();
}
