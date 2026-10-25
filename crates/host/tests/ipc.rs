//! A real socket, the engine behind `EngineOperations` (with the shared local-disk fake
//! transport), and the client.
#![cfg(unix)]

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use filebridge_core::{
    AppSettings, Engine, EngineOptions, Event, JobState, Profile, Recurrence, Registry,
    ScheduleInput, ScheduleOutcome, SendOptions, SendWindow, Store, TargetSpec, Weekday,
};
// Only the tests that need an NFC/NFD collision use this, and those don't run on macOS.
#[cfg(not(target_os = "macos"))]
use filebridge_core::DecisionInput;
use filebridge_host::EngineOperations;
#[cfg(not(target_os = "macos"))]
use filebridge_ipc::PROTOCOL_VERSION;
use filebridge_ipc::{Client, Endpoint, Operations, bind, serve};
use filebridge_transport_api::{Backend, Destination};
use filebridge_transport_fake::FakeTransport;
use interprocess::local_socket::tokio::prelude::*;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::broadcast::error::RecvError;

// Needs an NFC and an NFD name side by side: one name on macOS (APFS, HFS+).
#[cfg(not(target_os = "macos"))]
#[tokio::test]
async fn client_drives_a_job_over_the_socket() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("Fotos");
    let dest = dir.path().join("dest");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::create_dir_all(&dest).unwrap();
    std::fs::write(src.join("Vera\u{303}o.jpg"), b"x").unwrap();
    std::fs::write(src.join("Ver\u{e3}o.jpg"), b"y").unwrap(); // collides after NFC

    let mut registry = Registry::new();
    registry.add_transport(Arc::new(FakeTransport::default()));
    let engine = Engine::new(
        Store::open_in_memory().unwrap(),
        registry,
        EngineOptions {
            inhibit_sleep: false,
            ..EngineOptions::default()
        },
    );
    let endpoint = Endpoint::Path(dir.path().join("run/filebridge/service.sock"));
    let listener = bind(&endpoint).unwrap();
    tokio::spawn(serve(listener, Arc::new(EngineOperations::new(engine))));

    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(dir.path().join("run/filebridge"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o700, "socket directory is user-only");
    }

    let client = Client::connect(&endpoint).await.unwrap();
    assert_eq!(client.ping().await.unwrap().protocol, PROTOCOL_VERSION);
    {
        // "Copy log" (D07): the engine's recent lines, secret parameters scrubbed.
        use tracing_subscriber::layer::SubscriberExt;
        let subscriber =
            tracing_subscriber::registry().with(filebridge_host::RecentLog::global().layer());
        tracing::subscriber::with_default(subscriber, || {
            tracing::warn!("rclone: bad fs :sftp,host=example.invalid,pass=hunter2:/x");
        });
        let log = client.service_log().await.unwrap();
        let line = log
            .iter()
            .find(|l| l.contains("bad fs"))
            .expect("logged line");
        assert!(
            line.contains("pass=***") && !line.contains("hunter2"),
            "{line}"
        );
    }
    let mut events = client.events().await.unwrap();

    let profile = client
        .save_profile(Profile {
            id: String::new(),
            name: "dest".into(),
            destination: Destination {
                transport: FakeTransport::ID.into(),
                backend: Backend::Local,
                root: dest.to_string_lossy().into_owned(),
            },
            integration: None,
            integration_settings: serde_json::Value::Null,
            target: TargetSpec::default(),
        })
        .await
        .unwrap();
    assert_eq!(client.list_profiles().await.unwrap().len(), 1);

    let job = client
        .create_job(
            vec![src.to_string_lossy().into_owned()],
            profile.id.clone(),
            SendOptions::default(),
        )
        .await
        .unwrap();
    let summary = client.analyse(job.clone()).await.unwrap();
    assert_eq!(summary.state, JobState::AwaitingApproval);

    let err = client.start(job.clone()).await.unwrap_err();
    assert_eq!(err.code, "pending_decisions");

    let issues = client
        .issues(job.clone(), Some("collision".into()), None, 50, None)
        .await
        .unwrap();
    client
        .decide(job.clone(), issues[0].id, DecisionInput::Accept)
        .await
        .unwrap();
    client.start(job.clone()).await.unwrap();

    let finished = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if let Ok(Event::JobFinished { job_id, state, .. }) = events.recv().await
                && job_id == job
            {
                return state;
            }
        }
    })
    .await
    .expect("a JobFinished event arrives");
    assert_eq!(finished, JobState::Completed);
    assert_eq!(client.job(job.clone()).await.unwrap().counts.done, 2);
    assert!(dest.join("Fotos/Verão.jpg").exists());
    assert!(dest.join("Fotos/Verão (2).jpg").exists());

    let missing = client.job("nope".into()).await.unwrap_err();
    assert_eq!(missing.code, "not_found");
    let unsupported = client
        .detect_nextcloud(filebridge_ipc::DetectNextcloudRequest {
            name: "x".into(),
            host: "h".into(),
            port: 22,
            user: "u".into(),
            key_file: None,
            known_hosts_file: None,
            nextcloud_user: "n".into(),
            folder: String::new(),
            ctid: None,
            install_dir: None,
            dedicated_user: false,
        })
        .await
        .unwrap_err();
    assert_eq!(unsupported.code, "unsupported");
}

/// Operations whose own argument is called `id` answer over the socket: that argument once
/// collided with the request's id, the service refused the request under id 0, and the window
/// waited for its answer forever ("Testing the connection…").
#[tokio::test]
async fn operations_with_an_id_argument_answer_over_the_socket() {
    let dir = tempfile::tempdir().unwrap();
    let (_endpoint, client, profile) = service(dir.path()).await;
    let answer = Duration::from_secs(10);

    let test = tokio::time::timeout(answer, client.test_profile(profile.clone()))
        .await
        .expect("test_profile answers")
        .unwrap();
    assert!(!test.transport.has_failures(), "{test:?}");
    let space = tokio::time::timeout(answer, client.profile_storage(profile.clone(), true))
        .await
        .expect("profile_storage answers")
        .unwrap();
    assert_eq!(space.profile_id, profile);
    tokio::time::timeout(answer, client.delete_profile(profile))
        .await
        .expect("delete_profile answers")
        .unwrap();
    assert!(client.list_profiles().await.unwrap().is_empty());
}

/// A service on a fresh socket, with the copy transport and a profile writing to `dest`.
async fn service(dir: &Path) -> (Endpoint, Client, String) {
    let mut registry = Registry::new();
    registry.add_transport(Arc::new(FakeTransport::default()));
    let engine = Engine::new(
        Store::open_in_memory().unwrap(),
        registry,
        EngineOptions {
            inhibit_sleep: false,
            ..EngineOptions::default()
        },
    );
    let endpoint = Endpoint::Path(dir.join("run/service.sock"));
    tokio::spawn(serve(
        bind(&endpoint).unwrap(),
        Arc::new(EngineOperations::new(engine)),
    ));
    let client = Client::connect(&endpoint).await.unwrap();
    let dest = dir.join("dest");
    std::fs::create_dir_all(&dest).unwrap();
    let profile = client
        .save_profile(Profile {
            id: String::new(),
            name: "dest".into(),
            destination: Destination {
                transport: FakeTransport::ID.into(),
                backend: Backend::Local,
                root: dest.to_string_lossy().into_owned(),
            },
            integration: None,
            integration_settings: serde_json::Value::Null,
            target: TargetSpec::default(),
        })
        .await
        .unwrap();
    (endpoint, client, profile.id)
}

// Needs an NFC and an NFD name side by side: one name on macOS (APFS, HFS+).
#[cfg(not(target_os = "macos"))]
#[tokio::test]
async fn set_options_and_decide_all_over_the_socket() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("Fotos");
    std::fs::create_dir_all(&src).unwrap();
    // Two pairs that collide after NFC: two decisions.
    std::fs::write(src.join("Vera\u{303}o.jpg"), b"x").unwrap();
    std::fs::write(src.join("Ver\u{e3}o.jpg"), b"y").unwrap();
    std::fs::write(src.join("Inve\u{301}rno.jpg"), b"x").unwrap();
    std::fs::write(src.join("Inv\u{e9}rno.jpg"), b"y").unwrap();
    let (_endpoint, client, profile_id) = service(dir.path()).await;

    let job = client
        .create_job(
            vec![src.to_string_lossy().into_owned()],
            profile_id,
            SendOptions::default(),
        )
        .await
        .unwrap();
    let summary = client.analyse(job.clone()).await.unwrap();
    assert_eq!(summary.state, JobState::AwaitingApproval);
    assert_eq!(summary.pending_decisions, 2);

    let verify = SendOptions {
        verify: true,
        ..SendOptions::default()
    };
    client
        .set_options(job.clone(), verify.clone())
        .await
        .unwrap();
    let summary = client.job(job.clone()).await.unwrap();
    assert_eq!(summary.options, verify);
    assert_eq!(summary.state, JobState::AwaitingApproval);
    let missing = client
        .set_options("nope".into(), verify.clone())
        .await
        .unwrap_err();
    assert_eq!(missing.code, "not_found");

    let edit = client
        .decide_all(job.clone(), None, DecisionInput::Edit("x".into()), None)
        .await
        .unwrap_err();
    assert_eq!(edit.code, "invalid");
    let changed = client
        .decide_all(
            job.clone(),
            Some("collision".into()),
            DecisionInput::Accept,
            None,
        )
        .await
        .unwrap();
    assert_eq!(changed, 2);
    assert_eq!(
        client.job(job.clone()).await.unwrap().state,
        JobState::Ready
    );

    client.start(job.clone()).await.unwrap();
    let done = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let j = client.job(job.clone()).await.unwrap();
            if j.state.is_terminal() {
                return j;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the job finishes");
    // Verify was switched on after creation, and it ran.
    assert_eq!(done.state, JobState::Completed);
    assert_eq!(
        done.counts.verified_ok,
        done.counts.files - done.counts.skipped
    );
    let sent = client
        .set_options(job.clone(), SendOptions::default())
        .await
        .unwrap_err();
    assert_eq!(sent.code, "invalid");
}

#[tokio::test]
async fn event_streams_close_when_the_service_drops_the_connection() {
    let dir = tempfile::tempdir().unwrap();
    let endpoint = Endpoint::Path(dir.path().join("run/service.sock"));
    let listener = bind(&endpoint).unwrap();
    let (hang_up, when) = tokio::sync::oneshot::channel::<()>();
    // A service that accepts `subscribe`, sends one event and hangs up when told to.
    let server = tokio::spawn(async move {
        let conn = listener.accept().await.unwrap();
        let (recv, mut send) = conn.split();
        let mut lines = BufReader::new(recv).lines();
        let line = lines.next_line().await.unwrap().unwrap();
        let request: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert_eq!(request["request"]["type"], "subscribe");
        let answer = serde_json::json!({ "kind": "response", "id": request["id"], "ok": null });
        send.write_all(format!("{answer}\n").as_bytes())
            .await
            .unwrap();
        let event = serde_json::json!({ "kind": "event", "event": {
            "type": "job_finished", "job_id": "j", "state": "completed", "error": null,
        }});
        send.write_all(format!("{event}\n").as_bytes())
            .await
            .unwrap();
        when.await.unwrap();
        // Dropping both halves closes the connection.
    });

    let client = Client::connect(&endpoint).await.unwrap();
    let mut events = client.events().await.unwrap();
    let mut second = client.events().await.unwrap();
    hang_up.send(()).unwrap();
    server.await.unwrap();

    let next = tokio::time::timeout(Duration::from_secs(5), events.recv())
        .await
        .expect("the stream does not hang");
    assert!(matches!(next, Ok(Event::JobFinished { .. })), "{next:?}");
    let next = tokio::time::timeout(Duration::from_secs(5), events.recv())
        .await
        .expect("the stream does not hang");
    assert!(matches!(next, Err(RecvError::Closed)), "{next:?}");
    let next = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Err(e) = second.recv().await {
                return e;
            }
        }
    })
    .await
    .expect("every receiver closes");
    assert!(matches!(next, RecvError::Closed));

    // Later calls fail at once instead of waiting for an answer.
    assert_eq!(client.events().await.unwrap_err().code, "disconnected");
    let ping = tokio::time::timeout(Duration::from_secs(5), client.ping())
        .await
        .expect("a call on a dead connection does not hang");
    assert_eq!(ping.unwrap_err().code, "disconnected");
}

#[tokio::test]
async fn settings_over_the_socket() {
    let dir = tempfile::tempdir().unwrap();
    let (_endpoint, client, _profile_id) = service(dir.path()).await;
    assert_eq!(client.get_settings().await.unwrap(), AppSettings::default());

    let settings = AppSettings {
        bandwidth_limit: Some(5 * 1024 * 1024),
        send_window: Some(SendWindow {
            start_minute: 22 * 60,
            end_minute: 7 * 60,
        }),
    };
    client.save_settings(settings.clone()).await.unwrap();
    assert_eq!(client.get_settings().await.unwrap(), settings);

    let bad = AppSettings {
        send_window: Some(SendWindow {
            start_minute: 60,
            end_minute: 60,
        }),
        ..settings.clone()
    };
    assert_eq!(client.save_settings(bad).await.unwrap_err().code, "invalid");
    assert_eq!(client.get_settings().await.unwrap(), settings, "unchanged");
}

#[tokio::test]
async fn source_shortcuts_over_the_socket() {
    let dir = tempfile::tempdir().unwrap();
    let (_endpoint, client, _profile_id) = service(dir.path()).await;
    let fotos = dir.path().join("Fotos");
    for d in 0..20 {
        let sub = fotos.join(format!("{d:02}"));
        std::fs::create_dir_all(&sub).unwrap();
        for f in 0..100 {
            std::fs::write(sub.join(format!("{f}.jpg")), b"jpg").unwrap();
        }
    }
    let path = fotos.display().to_string();

    // Pins.
    client.pin_source(path.clone(), true).await.unwrap();
    let pins = client.pinned_sources().await.unwrap();
    assert_eq!(
        (pins.len(), pins[0].path.as_str(), pins[0].exists),
        (1, path.as_str(), true)
    );
    let refused = client
        .pin_source("relative".into(), true)
        .await
        .unwrap_err();
    assert_eq!(refused.code, "invalid");
    client.pin_source(path.clone(), false).await.unwrap();
    assert!(client.pinned_sources().await.unwrap().is_empty());

    // The disks mounted here: whatever they are, the call answers.
    client.volumes().await.unwrap();

    // A measurement streams its progress and stops when asked; requests on one connection
    // run side by side.
    let mut events = client.events().await.unwrap();
    let measuring = {
        let client = client.clone();
        let path = path.clone();
        tokio::spawn(async move { client.folder_sizes("w-1".into(), path).await })
    };
    let progress = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if let Ok(Event::FolderSizesProgress { scan_id, sizes }) = events.recv().await {
                return (scan_id, sizes);
            }
        }
    })
    .await
    .expect("progress arrives");
    assert_eq!(progress.0, "w-1");
    client.cancel_folder_sizes("w-1".into()).await.unwrap();
    let sizes = measuring.await.unwrap().unwrap();
    assert_eq!(sizes.path, path);
    if sizes.complete {
        assert_eq!(
            (sizes.files, sizes.bytes, sizes.items.len()),
            (2000, 6000, 20)
        );
    }
    let gone = client
        .folder_sizes("w-2".into(), dir.path().join("nope").display().to_string())
        .await
        .unwrap_err();
    assert_eq!(gone.code, "invalid");
}

#[tokio::test]
async fn schedules_over_the_socket() {
    let dir = tempfile::tempdir().unwrap();
    let (_endpoint, client, profile_id) = service(dir.path()).await;
    let src = dir.path().join("Fotos");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(src.join("a.jpg"), b"a").unwrap();
    let mut events = client.events().await.unwrap();

    let input = ScheduleInput {
        id: None,
        name: "Nightly".into(),
        sources: vec![src.to_string_lossy().into_owned()],
        profile_id: profile_id.clone(),
        options: SendOptions::default(),
        recurrence: Recurrence::Days {
            days: vec![Weekday::Mon, Weekday::Thu],
            minute: 3 * 60,
        },
        enabled: true,
    };
    let saved = client.save_schedule(input.clone()).await.unwrap();
    assert!(saved.next_run_at.is_some());
    assert!(saved.options.keep_older);
    assert_eq!(
        client.list_schedules().await.unwrap(),
        std::slice::from_ref(&saved)
    );

    let bad = ScheduleInput {
        recurrence: Recurrence::Hours {
            every: 0,
            minute: 0,
        },
        ..input.clone()
    };
    assert_eq!(client.save_schedule(bad).await.unwrap_err().code, "invalid");

    let job = client
        .run_schedule_now(saved.id.clone())
        .await
        .unwrap()
        .expect("a run with the new file");
    let ran = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if let Ok(Event::ScheduleRan {
                schedule_id,
                job_id,
                outcome,
            }) = events.recv().await
            {
                return (schedule_id, job_id, outcome);
            }
        }
    })
    .await
    .expect("a ScheduleRan event arrives");
    assert_eq!(
        ran,
        (
            saved.id.clone(),
            Some(job.clone()),
            ScheduleOutcome::Started
        )
    );
    let finished = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let j = client.job(job.clone()).await.unwrap();
            if j.state.is_terminal() {
                return j;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(finished.state, JobState::Completed);
    assert_eq!(finished.schedule_id.as_deref(), Some(saved.id.as_str()));
    assert_eq!(
        client.run_schedule_now(saved.id.clone()).await.unwrap(),
        None
    );

    client.delete_schedule(saved.id.clone()).await.unwrap();
    assert!(client.list_schedules().await.unwrap().is_empty());
    assert_eq!(
        client.delete_schedule(saved.id).await.unwrap_err().code,
        "not_found"
    );
}
