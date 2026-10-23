//! The integration against `MockImmich`, a local HTTP server that answers like Immich's API.

use std::sync::Arc;

use async_trait::async_trait;
use filebridge_integration_api::{
    CheckStatus, Integration, IntegrationEvent, IntegrationSink, JobContext, ProfileView,
    RemotePath,
};
use filebridge_integration_immich::fake::MockImmich;
use filebridge_integration_immich::{Immich, ImmichSettings, SCAN_STEP, SetupRequest};
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

fn immich(key: Option<&'static str>) -> Immich {
    Immich::new(Arc::new(OneKey(key)))
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
    let settings = ImmichSettings {
        server: server.into(),
        api_key: key_ref(),
        library_id: LIB.into(),
        import_path: import_path.into(),
    };
    JobContext {
        job_id: "job-1".into(),
        profile: ProfileView {
            id: "p1".into(),
            name: "Fotos".into(),
            destination: destination(),
            settings: serde_json::to_value(settings).unwrap(),
        },
        caps: TransportCaps::RESUME,
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
