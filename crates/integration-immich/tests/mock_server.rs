//! The integration against `MockImmich`, a local HTTP server that answers like Immich's API.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use filebridge_integration_api::{
    CheckStatus, Integration, IntegrationEvent, IntegrationSink, JobContext, ProfileView,
    RemotePath,
};
use filebridge_integration_immich::fake::{MockImmich, QueueApi};
use filebridge_integration_immich::{
    Immich, ImmichSettings, QUEUE_PAUSED, SAME_FOLDER, SCAN_NOT_FOLLOWED, SCAN_STEP,
    SCAN_UNFINISHED, ScanWait, SetupRequest, WAIT_STEP,
};
use filebridge_transport_api::{
    Backend, Destination, KeyringRef, SecretResolver, SecretValue, TransportCaps, TransportError,
};

const KEY: &str = "imm-key-0123456789abcdef";
const LIB: &str = "6f1c2a4e-0b7d-4c55-9a8e-3d2f1b0c9e77";
const OTHER: &str = "9a8b7c6d-5e4f-4a3b-8c2d-1e0f9a8b7c6d";

/// The keyring, as plugins see it: one stored key.
struct OneKey(Option<&'static str>);

#[async_trait]
impl SecretResolver for OneKey {
    async fn resolve(&self, r: &KeyringRef) -> filebridge_transport_api::Result<SecretValue> {
        match self.0 {
            Some(k) if r.0 == "filebridge/p1/api-key" => Ok(SecretValue::new(k.into())),
            _ => Err(TransportError::new(
                filebridge_transport_api::ErrorKind::Auth,
                format!("no secret stored for {}", r.0),
            )),
        }
    }
}

/// Follows scans every few milliseconds, for a second at most.
fn immich(key: Option<&'static str>) -> Immich {
    Immich::new(Arc::new(OneKey(key))).with_scan_wait(ScanWait {
        first_poll: Duration::from_millis(5),
        max_poll: Duration::from_millis(20),
        timeout: Duration::from_secs(1),
    })
}

fn settings(server: &str, import_path: &str) -> ImmichSettings {
    ImmichSettings {
        server: server.into(),
        api_key: key_ref(),
        library_id: LIB.into(),
        import_path: import_path.into(),
    }
}

fn paths(v: &[&str]) -> Vec<RemotePath> {
    v.iter().map(|p| RemotePath::new(*p).unwrap()).collect()
}

/// The events and the checkpoints an `after_transfer` call left; checkpoints are "saved".
async fn drain(
    mut rx: tokio::sync::mpsc::UnboundedReceiver<IntegrationEvent>,
    saves: tokio::task::JoinHandle<Vec<serde_json::Value>>,
) -> (Vec<IntegrationEvent>, Vec<serde_json::Value>) {
    let mut events = Vec::new();
    while let Some(e) = rx.recv().await {
        events.push(e);
    }
    (events, saves.await.unwrap())
}

fn checkpointing_sink() -> (
    IntegrationSink,
    tokio::sync::mpsc::UnboundedReceiver<IntegrationEvent>,
    tokio::task::JoinHandle<Vec<serde_json::Value>>,
) {
    let (sink, rx, mut saves) = IntegrationSink::with_checkpoints();
    let kept = tokio::spawn(async move {
        let mut kept = Vec::new();
        while let Some(c) = saves.recv().await {
            kept.push(c.state);
            let _ = c.saved.send(());
        }
        kept
    });
    (sink, rx, kept)
}

fn key_ref() -> KeyringRef {
    KeyringRef("filebridge/p1/api-key".into())
}

fn destination() -> Destination {
    Destination {
        transport: "rclone".into(),
        backend: Backend::Sftp {
            host: "nas.example.com".into(),
            port: None,
            user: Some("photos".into()),
            key_file: None,
            known_hosts_file: None,
            password: None,
            key: None,
        },
        root: "/srv/photos/Holidays".into(),
    }
}

fn ctx(server: &str, import_path: &str) -> JobContext {
    let settings = settings(server, import_path);
    JobContext {
        job_id: "job-1".into(),
        profile: ProfileView {
            id: "p1".into(),
            name: "Fotos".into(),
            destination: destination(),
            settings: serde_json::to_value(settings).unwrap(),
        },
        caps: TransportCaps::RESUME,
        checkpoint: None,
    }
}

async fn server() -> MockImmich {
    let mock = MockImmich::start(KEY).await;
    mock.add_library(LIB, "Fotos", &["/mnt/photos"]);
    mock.add_library(OTHER, "Scans", &["/mnt/scans"]);
    mock
}

fn status_of(checks: &[filebridge_integration_api::Check], id: &str) -> Option<CheckStatus> {
    checks.iter().find(|c| c.id == id).map(|c| c.status)
}

#[tokio::test]
async fn the_wizard_finds_the_library_by_the_folder() {
    let mock = server().await;
    let setup = immich(None)
        .set_up(&SetupRequest {
            server: mock.url(),
            api_key: KEY,
            api_key_ref: key_ref(),
            library_id: None,
            import_path: Some("/mnt/photos/Holidays/"),
            destination_root: "/srv/photos/Holidays",
        })
        .await
        .unwrap();
    let ids: Vec<_> = setup
        .checks
        .iter()
        .map(|c| (c.id.as_str(), c.status))
        .collect();
    assert_eq!(
        ids,
        [
            ("server", CheckStatus::Ok),
            ("api-key", CheckStatus::Ok),
            ("library", CheckStatus::Ok),
            ("import-path", CheckStatus::Ok),
            ("readable", CheckStatus::Ok),
        ]
    );
    assert!(
        setup.checks[0].message.contains("v1.132.3"),
        "{:?}",
        setup.checks
    );
    let s = setup.settings.expect("settings");
    assert_eq!(s.library_id, LIB);
    assert_eq!(s.import_path, "/mnt/photos/Holidays");
    assert_eq!(s.server, mock.url());
    assert_eq!(s.api_key, key_ref());
    s.check(&destination()).unwrap();
    // The key travels in its header, and only to the calls that need it.
    for r in mock.requests() {
        assert_eq!(
            r.api_key.is_some(),
            !r.path.starts_with("/api/server/"),
            "{r:?}"
        );
        assert!(!r.path.contains(KEY) && !r.body.contains(KEY), "{r:?}");
    }
}

#[tokio::test]
async fn without_an_import_path_the_destination_folder_is_used() {
    let mock = server().await;
    let setup = immich(None)
        .set_up(&SetupRequest {
            server: mock.url(),
            api_key: KEY,
            api_key_ref: key_ref(),
            library_id: Some(LIB),
            import_path: None,
            destination_root: "/mnt/photos/2024",
        })
        .await
        .unwrap();
    assert_eq!(setup.settings.unwrap().import_path, "/mnt/photos/2024");
}

#[tokio::test]
async fn a_folder_outside_every_library_lists_them() {
    let mock = server().await;
    let setup = immich(None)
        .set_up(&SetupRequest {
            server: mock.url(),
            api_key: KEY,
            api_key_ref: key_ref(),
            library_id: None,
            import_path: Some("/srv/photos"),
            destination_root: "/srv/photos",
        })
        .await
        .unwrap();
    assert!(setup.settings.is_none());
    assert_eq!(setup.candidates.len(), 2);
    let library = setup.checks.iter().find(|c| c.id == "library").unwrap();
    assert_eq!(library.status, CheckStatus::Fail);
    assert!(
        library.message.contains("/mnt/scans"),
        "{}",
        library.message
    );

    // Named, the library says what it imports instead.
    let setup = immich(None)
        .set_up(&SetupRequest {
            server: mock.url(),
            api_key: KEY,
            api_key_ref: key_ref(),
            library_id: Some(LIB),
            import_path: Some("/srv/photos"),
            destination_root: "/srv/photos",
        })
        .await
        .unwrap();
    assert!(setup.settings.is_none());
    assert_eq!(
        status_of(&setup.checks, "import-path"),
        Some(CheckStatus::Fail)
    );
}

#[tokio::test]
async fn two_libraries_holding_the_folder_ask_for_one() {
    let mock = server().await;
    mock.add_library(
        "11111111-2222-4333-8444-555555555555",
        "Everything",
        &["/mnt"],
    );
    let setup = immich(None)
        .set_up(&SetupRequest {
            server: mock.url(),
            api_key: KEY,
            api_key_ref: key_ref(),
            library_id: None,
            import_path: Some("/mnt/photos"),
            destination_root: "/srv/photos",
        })
        .await
        .unwrap();
    assert!(setup.settings.is_none());
    assert_eq!(setup.candidates.len(), 2);
    let library = setup.checks.iter().find(|c| c.id == "library").unwrap();
    assert!(
        library.message.contains("choose one"),
        "{}",
        library.message
    );
}

#[tokio::test]
async fn a_wrong_key_or_a_non_admin_fails_the_key_step() {
    let mock = server().await;
    let req = |key| SetupRequest {
        server: mock.url(),
        api_key: key,
        api_key_ref: key_ref(),
        library_id: Some(LIB),
        import_path: Some("/mnt/photos"),
        destination_root: "/srv/photos",
    };
    let setup = immich(None).set_up(&req("wrong-key")).await.unwrap();
    assert!(setup.settings.is_none());
    let key = setup.checks.iter().find(|c| c.id == "api-key").unwrap();
    assert_eq!(key.status, CheckStatus::Fail);
    assert!(key.message.contains("refused"), "{}", key.message);
    assert!(!key.message.contains("wrong-key"), "{}", key.message);

    mock.set_admin(false);
    let setup = immich(None).set_up(&req(KEY)).await.unwrap();
    let key = setup.checks.iter().find(|c| c.id == "api-key").unwrap();
    assert_eq!(key.status, CheckStatus::Fail);
    assert!(key.message.contains("administrator"), "{}", key.message);
}

#[tokio::test]
async fn bad_input_is_refused_before_anything_is_asked() {
    let mock = server().await;
    let base = SetupRequest {
        server: mock.url(),
        api_key: KEY,
        api_key_ref: key_ref(),
        library_id: None,
        import_path: None,
        destination_root: "/srv/photos",
    };
    for req in [
        SetupRequest {
            server: "http://192.0.2.10:2283",
            ..base.clone()
        },
        SetupRequest {
            library_id: Some("../users"),
            ..base.clone()
        },
        SetupRequest {
            import_path: Some("mnt/photos"),
            ..base.clone()
        },
        SetupRequest {
            api_key: " ",
            ..base.clone()
        },
    ] {
        assert!(immich(None).set_up(&req).await.is_err());
    }
    assert!(mock.requests().is_empty());
}

#[tokio::test]
async fn nothing_answering_fails_the_server_step() {
    let mock = MockImmich::start(KEY).await;
    let url = mock.url().to_owned();
    drop(mock);
    tokio::task::yield_now().await;
    let report = immich(Some(KEY))
        .preflight(&ctx(&url, "/mnt/photos"))
        .await
        .unwrap();
    assert_eq!(report.checks.len(), 1);
    assert_eq!(report.checks[0].id, "server");
    assert_eq!(report.checks[0].status, CheckStatus::Fail);
}

#[tokio::test]
async fn preflight_checks_the_saved_profile() {
    let mock = server().await;
    let report = immich(Some(KEY))
        .preflight(&ctx(mock.url(), "/mnt/photos/Holidays"))
        .await
        .unwrap();
    assert!(
        report.checks.iter().all(|c| c.status == CheckStatus::Ok),
        "{:?}",
        report.checks
    );
    assert!(report.latency_ms.is_some());

    // Immich can't read the folder: a warning, the files still go.
    mock.set_unreadable(LIB);
    let report = immich(Some(KEY))
        .preflight(&ctx(mock.url(), "/mnt/photos/Holidays"))
        .await
        .unwrap();
    assert_eq!(
        status_of(&report.checks, "readable"),
        Some(CheckStatus::Warn)
    );

    // The key is gone from the keyring.
    let report = immich(None)
        .preflight(&ctx(mock.url(), "/mnt/photos"))
        .await
        .unwrap();
    assert_eq!(
        status_of(&report.checks, "api-key"),
        Some(CheckStatus::Fail)
    );
}

#[tokio::test]
async fn after_the_send_the_library_is_scanned() {
    let mock = server().await;
    let (sink, mut rx) = IntegrationSink::channel();
    let changed = [
        RemotePath::new("2024/a.jpg").unwrap(),
        RemotePath::new("2024/b.heic").unwrap(),
    ];
    let report = immich(Some(KEY))
        .after_transfer(&ctx(mock.url(), "/mnt/photos/Holidays"), &changed, sink)
        .await
        .unwrap();
    assert_eq!(report.items, 2);
    assert_eq!(report.updated, ["/mnt/photos/Holidays"]);
    assert!(report.is_clean());
    assert_eq!(mock.scans(), [format!("/api/libraries/{LIB}/scan")]);
    assert_eq!(
        rx.recv().await,
        Some(IntegrationEvent::Stage(SCAN_STEP.into()))
    );
}

#[tokio::test]
async fn the_step_follows_the_scan_until_immich_is_done() {
    let mock = server().await;
    mock.set_scan_polls(4);
    mock.set_imported_per_scan(2);
    let (sink, rx, saves) = checkpointing_sink();
    let changed = paths(&["2024/a.jpg", "2024/b.heic"]);
    let report = immich(Some(KEY))
        .after_transfer(&ctx(mock.url(), "/mnt/photos/Holidays"), &changed, sink)
        .await
        .unwrap();
    assert!(report.is_clean(), "{:?}", report.problems);
    let (events, kept) = drain(rx, saves).await;
    assert_eq!(
        kept,
        [serde_json::json!({ "scan_requested": true, "assets_before": 42 })],
        "saved once Immich accepted the scan"
    );
    assert!(events.contains(&IntegrationEvent::Stage(WAIT_STEP.into())));
    let looks = mock
        .requests()
        .iter()
        .filter(|r| r.path == "/api/queues/library")
        .count();
    assert_eq!(looks, 4, "until the queue is idle");
    let imported: Vec<u64> = events
        .iter()
        .filter_map(|e| match e {
            IntegrationEvent::Progress(p) if p.step == WAIT_STEP => Some(p.items),
            _ => None,
        })
        .collect();
    assert_eq!(imported, [0, 0, 0], "{events:?}");
    assert_eq!(mock.scans().len(), 1);
}

#[tokio::test]
async fn an_older_immich_is_followed_through_its_jobs_api() {
    let mock = server().await;
    mock.set_queue_api(QueueApi::Legacy);
    mock.set_scan_polls(3);
    let report = immich(Some(KEY))
        .after_transfer(
            &ctx(mock.url(), "/mnt/photos"),
            &paths(&["a.jpg"]),
            IntegrationSink::noop(),
        )
        .await
        .unwrap();
    assert!(report.is_clean(), "{:?}", report.problems);
    let jobs = mock
        .requests()
        .iter()
        .filter(|r| r.path == "/api/jobs")
        .count();
    assert_eq!(jobs, 3);
}

#[tokio::test]
async fn a_resumed_step_waits_for_its_scan_instead_of_asking_again() {
    let mock = server().await;
    // The first run asked for the scan and saved that, then the app stopped; Immich scans on.
    mock.start_scan(LIB, 3);
    let mut c = ctx(mock.url(), "/mnt/photos");
    c.checkpoint = Some(serde_json::json!({ "scan_requested": true, "assets_before": 40 }));
    let (sink, rx, saves) = checkpointing_sink();
    let report = immich(Some(KEY))
        .after_transfer(&c, &paths(&["a.jpg"]), sink)
        .await
        .unwrap();
    assert!(report.is_clean(), "{:?}", report.problems);
    assert!(mock.scans().is_empty(), "no second scan");
    let (events, kept) = drain(rx, saves).await;
    assert!(kept.is_empty());
    assert!(!events.contains(&IntegrationEvent::Stage(SCAN_STEP.into())));
    assert!(
        events.iter().any(|e| matches!(e,
            IntegrationEvent::Progress(p) if p.step == WAIT_STEP && p.items == 2)),
        "42 assets now, 40 before: {events:?}"
    );
}

#[tokio::test]
async fn a_scan_that_doesnt_finish_in_time_is_a_problem_not_a_failure() {
    let mock = server().await;
    mock.set_queue_paused(true);
    let (sink, rx, saves) = checkpointing_sink();
    let report = immich(Some(KEY))
        .after_transfer(&ctx(mock.url(), "/mnt/photos"), &paths(&["a.jpg"]), sink)
        .await
        .unwrap();
    let codes: Vec<_> = report.problems.iter().map(|p| p.code.as_str()).collect();
    assert_eq!(codes, [SCAN_UNFINISHED]);
    assert!(
        report.problems[0].message.contains("1 min"),
        "{:?}",
        report.problems
    );
    let (events, _) = drain(rx, saves).await;
    assert!(events.contains(&IntegrationEvent::Waiting(QUEUE_PAUSED.into())));
}

#[tokio::test]
async fn a_key_that_may_not_read_the_queue_says_so() {
    let mock = server().await;
    mock.set_queue_api(QueueApi::Status(403));
    let report = immich(Some(KEY))
        .after_transfer(
            &ctx(mock.url(), "/mnt/photos"),
            &paths(&["a.jpg"]),
            IntegrationSink::noop(),
        )
        .await
        .unwrap();
    assert_eq!(report.problems.len(), 1);
    assert_eq!(report.problems[0].code, SCAN_NOT_FOLLOWED);
    assert!(report.problems[0].message.contains("queue.read"));
    assert_eq!(mock.scans().len(), 1, "the scan was still asked for");
}

#[tokio::test]
async fn files_immich_will_skip_are_listed_before_and_after() {
    let mock = server().await;
    mock.set_exclusion_patterns(LIB, &["**/@eaDir/**", "**/Private/**"]);
    let c = ctx(mock.url(), "/mnt/photos/Holidays");
    let files = paths(&[
        "2024/a.jpg",
        "2024/a.xmp",
        "2024/notes.txt",
        "2024/clip.MOV",
        "Private/b.jpg",
        "@eaDir/a.jpg@SYNOPHOTO_THUMB_M.jpg",
        ".thumbs/c.jpg",
        "2024/scan.tiff",
    ]);
    let im = immich(Some(KEY));
    let skipped = im.will_skip(&c, &files).await.unwrap();
    let got: Vec<_> = skipped
        .iter()
        .map(|p| (p.path.as_deref().unwrap(), p.code.as_str()))
        .collect();
    assert_eq!(
        got,
        [
            ("2024/notes.txt", "immich.unsupported"),
            ("Private/b.jpg", "immich.excluded"),
            ("@eaDir/a.jpg@SYNOPHOTO_THUMB_M.jpg", "immich.excluded"),
            (".thumbs/c.jpg", "immich.hidden"),
            // The mock's media types don't list .tiff.
            ("2024/scan.tiff", "immich.unsupported"),
        ]
    );

    // An Immich without /server/media-types: Immich's own lists, which have .tiff.
    mock.set_no_media_types();
    let skipped = im.will_skip(&c, &files).await.unwrap();
    assert_eq!(skipped.len(), 4, "{skipped:?}");

    let report = im
        .after_transfer(&c, &files, IntegrationSink::noop())
        .await
        .unwrap();
    assert_eq!(report.problems.len(), 4, "{:?}", report.problems);
    assert_eq!(report.items, 4, "the files Immich takes");
    assert_eq!(mock.scans().len(), 1, "skipped files don't stop the scan");
}

#[tokio::test]
async fn the_probe_file_shows_whether_immich_sees_the_folder() {
    let mock = server().await;
    let dir = tempfile::tempdir().unwrap();
    let probe = RemotePath::new(".filebridge-probe-0d1e").unwrap();
    std::fs::write(dir.path().join(probe.as_str()), b"probe").unwrap();
    let im = immich(Some(KEY));
    let s = settings(mock.url(), "/mnt/photos/Holidays");

    // Immich's container mounts this folder at /mnt/photos/Holidays.
    mock.mount("/mnt/photos/Holidays", dir.path());
    let check = im
        .same_folder(&s, KEY, "/srv/photos/Holidays", &probe)
        .await;
    assert_eq!(check.id, SAME_FOLDER);
    assert_eq!(check.status, CheckStatus::Ok, "{}", check.message);

    // It mounts another folder there: the probe isn't where Immich looks.
    let elsewhere = tempfile::tempdir().unwrap();
    let mock = server().await;
    mock.mount("/mnt/photos/Holidays", elsewhere.path());
    let s = settings(mock.url(), "/mnt/photos/Holidays");
    let check = im
        .same_folder(&s, KEY, "/srv/photos/Holidays", &probe)
        .await;
    assert_eq!(check.status, CheckStatus::Fail);
    assert!(
        check.message.contains("/srv/photos/Holidays") && check.message.contains("/mnt/photos"),
        "{}",
        check.message
    );
    let validated = mock
        .requests()
        .into_iter()
        .rfind(|r| r.path.ends_with("/validate"))
        .unwrap();
    assert!(
        validated
            .body
            .contains("/mnt/photos/Holidays/.filebridge-probe-0d1e"),
        "{}",
        validated.body
    );

    // Immich doesn't answer the question: can't tell, a warning.
    let check = im
        .same_folder(&s, "wrong", "/srv/photos/Holidays", &probe)
        .await;
    assert_eq!(check.status, CheckStatus::Warn);
}

#[tokio::test]
async fn a_busy_immich_is_retried_and_a_refusal_is_not() {
    let mock = server().await;
    let changed = [RemotePath::new("a.jpg").unwrap()];
    let c = ctx(mock.url(), "/mnt/photos");

    mock.set_scan_status(503);
    let err = immich(Some(KEY))
        .after_transfer(&c, &changed, IntegrationSink::noop())
        .await
        .unwrap_err();
    assert!(err.retryable, "{err}");

    mock.set_scan_status(403);
    let err = immich(Some(KEY))
        .after_transfer(&c, &changed, IntegrationSink::noop())
        .await
        .unwrap_err();
    assert!(!err.retryable);
    assert!(err.message.contains("library.update"), "{err}");

    let err = immich(None)
        .after_transfer(&c, &changed, IntegrationSink::noop())
        .await
        .unwrap_err();
    assert!(!err.retryable, "a missing key is not retried: {err}");
}

#[tokio::test]
async fn check_profile_runs_the_settings_checks() {
    let mock = server().await;
    let c = ctx(mock.url(), "/mnt/photos");
    let im = immich(None);
    im.check_profile(&destination(), &c.profile.settings)
        .unwrap();
    assert!(
        im.check_profile(&destination(), &serde_json::json!({ "server": "x" }))
            .is_err()
    );
    assert!(im.name_rules(&c.profile.settings).is_empty());
}
