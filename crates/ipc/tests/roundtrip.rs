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
use filebridge_ipc::{Client, Endpoint, bind, serve};
use filebridge_transport_api::{
    Backend, Destination, Digest, HashAlgo, ItemOutcome, PreflightReport, ProgressEvent,
    ProgressSink, RemotePath, Result, TransferJob, TransferResult, Transport, TransportCaps,
    TransportError,
};

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
    assert_eq!(client.ping().await.unwrap().protocol, 1);
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
