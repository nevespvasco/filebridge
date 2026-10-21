//! A real socket, a real engine (with a local-copy fake transport), and the client.
#![cfg(unix)]

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use filebridge_core::{
    DecisionInput, Engine, EngineOptions, Event, JobState, Profile, Registry, SendOptions, Store,
    TargetSpec,
};
use filebridge_ipc::{Client, Endpoint, PROTOCOL_VERSION, bind, serve};
use filebridge_transport_api::{
    Backend, Destination, Digest, HashAlgo, ItemOutcome, PreflightReport, ProgressEvent,
    ProgressSink, RemotePath, Result, TransferJob, TransferResult, Transport, TransportCaps,
    TransportError,
};
use interprocess::local_socket::tokio::prelude::*;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::broadcast::error::RecvError;

struct CopyTransport;

#[async_trait]
impl Transport for CopyTransport {
    fn id(&self) -> &str {
        "copy"
    }
    fn capabilities(&self) -> TransportCaps {
        TransportCaps::RESUME
    }
    async fn preflight(&self, _dest: &Destination) -> Result<PreflightReport> {
        Ok(PreflightReport::default())
    }
    async fn transfer(
        &self,
        dest: &Destination,
        job: &TransferJob,
        progress: ProgressSink,
    ) -> Result<TransferResult> {
        let mut items = Vec::new();
        for (i, item) in job.items.iter().enumerate() {
            let to = Path::new(&dest.root).join(item.dest.as_str());
            std::fs::create_dir_all(to.parent().unwrap()).unwrap();
            std::fs::copy(job.source_root.join(&item.source), &to).unwrap();
            progress.send(ProgressEvent::ItemDone { index: i, ok: true });
            items.push(ItemOutcome::Done);
        }
        Ok(TransferResult { items, bytes: 0 })
    }
    async fn hash(&self, _: &Destination, _: &RemotePath, _: HashAlgo) -> Result<Digest> {
        Err(TransportError::fatal("no hashes"))
    }
}

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
    registry.add_transport(Arc::new(CopyTransport));
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
    tokio::spawn(serve(listener, engine, None));

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
    let mut events = client.events().await.unwrap();

    let profile = client
        .save_profile(Profile {
            id: String::new(),
            name: "dest".into(),
            destination: Destination {
                transport: "copy".into(),
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
        .issues(job.clone(), Some("collision".into()), None, 50)
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
        })
        .await
        .unwrap_err();
    assert_eq!(unsupported.code, "unsupported");
}

/// A service on a fresh socket, with the copy transport and a profile writing to `dest`.
async fn service(dir: &Path) -> (Endpoint, Client, String) {
    let mut registry = Registry::new();
    registry.add_transport(Arc::new(CopyTransport));
    let engine = Engine::new(
        Store::open_in_memory().unwrap(),
        registry,
        EngineOptions {
            inhibit_sleep: false,
            ..EngineOptions::default()
        },
    );
    let endpoint = Endpoint::Path(dir.join("run/service.sock"));
    tokio::spawn(serve(bind(&endpoint).unwrap(), engine, None));
    let client = Client::connect(&endpoint).await.unwrap();
    let dest = dir.join("dest");
    std::fs::create_dir_all(&dest).unwrap();
    let profile = client
        .save_profile(Profile {
            id: String::new(),
            name: "dest".into(),
            destination: Destination {
                transport: "copy".into(),
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
        .decide_all(job.clone(), None, DecisionInput::Edit("x".into()))
        .await
        .unwrap_err();
    assert_eq!(edit.code, "invalid");
    let changed = client
        .decide_all(job.clone(), Some("collision".into()), DecisionInput::Accept)
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
    // Verify was switched on after creation; this transport can't hash, hence the warnings.
    assert_eq!(done.state, JobState::CompletedWithWarnings);
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
        assert_eq!(request["type"], "subscribe");
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
