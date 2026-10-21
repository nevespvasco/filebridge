//! End-to-end engine tests with a local-disk fake transport and a recording fake integration.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use filebridge_core::backoff::Backoff;
use filebridge_core::{
    CoreError, DecisionInput, Engine, EngineOptions, Event, JobState, Profile, Registry,
    SendOptions, Store, StoreError, TargetSpec,
};
use filebridge_integration_api::{
    AfterTransferReport, Integration, IntegrationEvent, IntegrationSink, JobContext, NameRule,
    Problem, StepProgress,
};
use filebridge_transport_api::{
    Backend, Destination, Digest, HashAlgo, ItemOutcome, PreflightReport, ProgressEvent,
    ProgressSink, RemotePath, Result as TResult, StatsSnapshot, TransferJob, TransferResult,
    Transport, TransportCaps, TransportError,
};
use sha2::{Digest as _, Sha256};

#[derive(Default)]
struct FakeTransport {
    /// Whole transfer calls that fail with a retryable error before succeeding.
    fail_calls: AtomicU32,
    /// File names that always fail with a fatal error.
    fatal_names: Mutex<HashSet<String>>,
    /// File names written corrupted the first time.
    corrupt_once: Mutex<HashSet<String>>,
    /// Delay per file, to make pausing observable.
    per_file: Mutex<Duration>,
    calls: AtomicU32,
    /// The destination can't compute hashes.
    no_hash: std::sync::atomic::AtomicBool,
}

#[async_trait]
impl Transport for FakeTransport {
    fn id(&self) -> &str {
        "fake"
    }

    fn capabilities(&self) -> TransportCaps {
        TransportCaps::RESUME | TransportCaps::HASH_SHA256 | TransportCaps::RENAME
    }

    async fn preflight(&self, _dest: &Destination) -> TResult<PreflightReport> {
        Ok(PreflightReport {
            free_bytes: Some(1 << 40),
            writable: true,
            ..Default::default()
        })
    }

    async fn transfer(
        &self,
        dest: &Destination,
        job: &TransferJob,
        progress: ProgressSink,
    ) -> TResult<TransferResult> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if self
            .fail_calls
            .try_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1))
            .is_ok()
        {
            return Err(TransportError::retryable("connection reset by peer"));
        }
        let delay = *self.per_file.lock().unwrap();
        let mut items = Vec::new();
        let mut bytes = 0;
        for (i, item) in job.items.iter().enumerate() {
            if !delay.is_zero() {
                tokio::time::sleep(delay).await;
            }
            let name = item.dest.file_name().to_owned();
            if self.fatal_names.lock().unwrap().contains(&name) {
                items.push(ItemOutcome::Failed(TransportError::fatal(
                    "permission denied",
                )));
                continue;
            }
            let from = job.source_root.join(&item.source);
            let to = Path::new(&dest.root).join(item.dest.as_str());
            std::fs::create_dir_all(to.parent().unwrap()).unwrap();
            if self.corrupt_once.lock().unwrap().remove(&name) {
                std::fs::write(&to, b"garbage").unwrap();
            } else {
                std::fs::copy(&from, &to).unwrap();
            }
            bytes += item.size;
            progress.send(ProgressEvent::ItemDone { index: i, ok: true });
            progress.send(ProgressEvent::Stats(StatsSnapshot {
                bytes,
                files_done: i as u64 + 1,
                bytes_per_sec: 1000.0,
                ..Default::default()
            }));
            items.push(ItemOutcome::Done);
        }
        Ok(TransferResult { items, bytes })
    }

    async fn hash(&self, dest: &Destination, path: &RemotePath, algo: HashAlgo) -> TResult<Digest> {
        if self.no_hash.load(Ordering::SeqCst) {
            return Err(TransportError::fatal(
                "hash unsupported: hash type not supported",
            ));
        }
        let data = std::fs::read(Path::new(&dest.root).join(path.as_str()))
            .map_err(|e| TransportError::fatal(e.to_string()))?;
        Ok(Digest {
            algo,
            bytes: Sha256::digest(&data).to_vec(),
        })
    }
}

#[derive(Default)]
struct FakeIntegration {
    changed: Mutex<Vec<String>>,
    problem: Mutex<Option<Problem>>,
}

#[async_trait]
impl Integration for FakeIntegration {
    fn id(&self) -> &str {
        "fakeint"
    }

    fn required_caps(&self) -> TransportCaps {
        TransportCaps::empty()
    }

    fn name_rules(&self) -> Vec<Box<dyn NameRule>> {
        Vec::new()
    }

    async fn preflight(
        &self,
        _ctx: &JobContext,
    ) -> filebridge_integration_api::Result<PreflightReport> {
        Ok(PreflightReport::default())
    }

    async fn after_transfer(
        &self,
        _ctx: &JobContext,
        changed: &[RemotePath],
        progress: IntegrationSink,
    ) -> filebridge_integration_api::Result<AfterTransferReport> {
        progress.send(IntegrationEvent::Stage("fake.scan".into()));
        progress.send(IntegrationEvent::Progress(StepProgress {
            step: "fake.scan".into(),
            done: 1,
            total: 1,
            items: changed.len() as u64,
            current: None,
        }));
        *self.changed.lock().unwrap() = changed.iter().map(|p| p.to_string()).collect();
        Ok(AfterTransferReport {
            updated: vec!["all".into()],
            items: changed.len() as u64,
            problems: self.problem.lock().unwrap().iter().cloned().collect(),
        })
    }
}

struct Setup {
    engine: Arc<Engine>,
    store: Store,
    transport: Arc<FakeTransport>,
    integration: Arc<FakeIntegration>,
    src: PathBuf,
    dest: PathBuf,
    profile_id: String,
    _dir: tempfile::TempDir,
}

fn engine_with(
    store: Store,
    transport: Arc<FakeTransport>,
    integration: Arc<FakeIntegration>,
) -> Arc<Engine> {
    engine_with_batch(store, transport, integration, 3)
}

fn engine_with_batch(
    store: Store,
    transport: Arc<FakeTransport>,
    integration: Arc<FakeIntegration>,
    batch_size: u32,
) -> Arc<Engine> {
    let mut registry = Registry::new();
    registry.add_transport(transport);
    registry.add_integration(integration);
    Engine::new(
        store,
        registry,
        EngineOptions {
            backoff: Backoff::Fixed(Duration::from_millis(10)),
            batch_size,
            inhibit_sleep: false,
            max_verify_rounds: 3,
        },
    )
}

async fn setup() -> Setup {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("src/Acme");
    let dest = dir.path().join("dest");
    std::fs::create_dir_all(src.join("Fotos")).unwrap();
    std::fs::create_dir_all(&dest).unwrap();
    // NFD names, as from macOS/iOS.
    std::fs::write(src.join("Fotos/Vera\u{303}o.jpg"), b"summer").unwrap();
    std::fs::write(src.join("Fotos/Inverno.jpg"), b"winter").unwrap();
    std::fs::write(src.join("Relato\u{301}rio.pdf"), b"report").unwrap();

    let store = Store::open_in_memory().unwrap();
    let transport = Arc::new(FakeTransport::default());
    let integration = Arc::new(FakeIntegration::default());
    let engine = engine_with(store.clone(), transport.clone(), integration.clone());
    let profile = engine
        .save_profile(Profile {
            id: String::new(),
            name: "Test".into(),
            destination: Destination {
                transport: "fake".into(),
                backend: Backend::Local,
                root: dest.to_string_lossy().into_owned(),
            },
            integration: Some("fakeint".into()),
            integration_settings: serde_json::json!({}),
            target: TargetSpec::default(),
        })
        .await
        .unwrap();
    Setup {
        engine,
        store,
        transport,
        integration,
        src,
        dest,
        profile_id: profile.id,
        _dir: dir,
    }
}

async fn ready_job(s: &Setup, options: SendOptions) -> String {
    let job = s
        .engine
        .create_job(vec![s.src.clone()], &s.profile_id, options)
        .await
        .unwrap();
    let summary = s.engine.analyse(&job).await.unwrap();
    assert_eq!(summary.state, JobState::Ready, "{summary:?}");
    job
}

async fn run_to_end(s: &Setup, job: &str) -> JobState {
    s.engine.start(job).await.unwrap();
    tokio::time::timeout(Duration::from_secs(20), s.engine.wait(job))
        .await
        .expect("job finishes")
        .unwrap()
}

fn dest_files(dest: &Path) -> Vec<String> {
    let mut out = Vec::new();
    for e in walk(dest) {
        out.push(e.strip_prefix(dest).unwrap().to_string_lossy().into_owned());
    }
    out.sort();
    out
}

fn walk(p: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for e in std::fs::read_dir(p).unwrap() {
        let path = e.unwrap().path();
        if path.is_dir() {
            out.extend(walk(&path));
        } else {
            out.push(path);
        }
    }
    out
}

#[tokio::test]
async fn sends_with_nfc_names_and_updates_the_integration() {
    let s = setup().await;
    let mut events = s.engine.subscribe();
    let job = ready_job(&s, SendOptions::default()).await;
    let rules = s.engine.rule_counts(&job).await.unwrap();
    assert_eq!(rules.len(), 1);
    assert_eq!((rules[0].rule.as_str(), rules[0].total), ("nfc", 2));

    assert_eq!(run_to_end(&s, &job).await, JobState::Completed);
    assert_eq!(
        dest_files(&s.dest),
        vec![
            "Acme/Fotos/Inverno.jpg".to_owned(),
            "Acme/Fotos/Verão.jpg".to_owned(),
            "Acme/Relatório.pdf".to_owned(),
        ]
    );
    let mut changed = s.integration.changed.lock().unwrap().clone();
    changed.sort();
    assert_eq!(changed.len(), 3);
    assert!(changed.contains(&"Acme/Fotos/Verão.jpg".to_owned()));

    let summary = s.engine.job(&job).await.unwrap();
    assert_eq!((summary.counts.done, summary.counts.files), (3, 3));
    assert_eq!(summary.counts.bytes_done, 18);

    let mut saw_progress = false;
    let mut saw_integration = false;
    while let Ok(e) = events.try_recv() {
        match e {
            Event::TransferProgress { .. } => saw_progress = true,
            Event::IntegrationProgress { .. } => saw_integration = true,
            _ => {}
        }
    }
    assert!(saw_progress && saw_integration);
}

#[tokio::test]
async fn collisions_wait_for_a_decision() {
    let s = setup().await;
    // Same visible name twice: NFC and NFD.
    std::fs::write(s.src.join("Fotos/Ver\u{e3}o.jpg"), b"other summer").unwrap();
    let job = s
        .engine
        .create_job(vec![s.src.clone()], &s.profile_id, SendOptions::default())
        .await
        .unwrap();
    let summary = s.engine.analyse(&job).await.unwrap();
    assert_eq!(summary.state, JobState::AwaitingApproval);
    assert_eq!(summary.pending_decisions, 1);
    assert!(s.engine.start(&job).await.is_err());

    let issues = s
        .engine
        .issues(&job, Some("collision".into()), None, 10)
        .await
        .unwrap();
    assert_eq!(issues.len(), 1);
    assert_eq!(issues[0].after_text, "Verão (2).jpg");
    s.engine
        .decide_issue(&job, issues[0].id, DecisionInput::Accept)
        .await
        .unwrap();
    assert_eq!(s.engine.job(&job).await.unwrap().state, JobState::Ready);

    assert_eq!(run_to_end(&s, &job).await, JobState::Completed);
    let files = dest_files(&s.dest);
    assert!(files.contains(&"Acme/Fotos/Verão.jpg".to_owned()));
    assert!(files.contains(&"Acme/Fotos/Verão (2).jpg".to_owned()));
}

#[tokio::test]
async fn retries_with_backoff_until_it_works() {
    let s = setup().await;
    s.transport.fail_calls.store(2, Ordering::SeqCst);
    let mut events = s.engine.subscribe();
    let job = ready_job(&s, SendOptions::default()).await;
    assert_eq!(run_to_end(&s, &job).await, JobState::Completed);
    assert_eq!(dest_files(&s.dest).len(), 3);
    let mut retries = 0;
    while let Ok(e) = events.try_recv() {
        if let Event::RetryScheduled { attempt, .. } = e {
            retries += 1;
            assert_eq!(attempt, retries);
        }
    }
    assert_eq!(retries, 2);
    assert_eq!(
        s.engine.job(&job).await.unwrap().attempt,
        0,
        "reset after progress"
    );
}

#[tokio::test]
async fn fatal_file_errors_finish_with_warnings() {
    let s = setup().await;
    s.transport
        .fatal_names
        .lock()
        .unwrap()
        .insert("Inverno.jpg".into());
    let job = ready_job(&s, SendOptions::default()).await;
    assert_eq!(run_to_end(&s, &job).await, JobState::CompletedWithWarnings);
    let summary = s.engine.job(&job).await.unwrap();
    assert_eq!((summary.counts.done, summary.counts.failed), (2, 1));
    assert!(summary.error.unwrap().contains("could not be sent"));
}

#[tokio::test]
async fn verification_resends_only_mismatches() {
    let s = setup().await;
    s.transport
        .corrupt_once
        .lock()
        .unwrap()
        .insert("Inverno.jpg".into());
    let job = ready_job(
        &s,
        SendOptions {
            verify: true,
            ..SendOptions::default()
        },
    )
    .await;
    let calls_before = s.transport.calls.load(Ordering::SeqCst);
    assert_eq!(run_to_end(&s, &job).await, JobState::Completed);
    assert_eq!(
        std::fs::read(s.dest.join("Acme/Fotos/Inverno.jpg")).unwrap(),
        b"winter"
    );
    let summary = s.engine.job(&job).await.unwrap();
    assert_eq!(
        (summary.counts.verified_ok, summary.counts.mismatch),
        (3, 0)
    );
    // 3 files in batches of 3 = 1 call, plus 1 call to resend the mismatch.
    assert_eq!(s.transport.calls.load(Ordering::SeqCst) - calls_before, 2);
}

#[tokio::test]
async fn pause_keeps_progress_and_resume_finishes() {
    let s = setup().await;
    for i in 0..12 {
        std::fs::write(s.src.join(format!("f{i:02}.txt")), b"x").unwrap();
    }
    *s.transport.per_file.lock().unwrap() = Duration::from_millis(30);
    let job = ready_job(&s, SendOptions::default()).await;
    s.engine.start(&job).await.unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;
    s.engine.pause(&job).await.unwrap();
    let paused = s.engine.job(&job).await.unwrap();
    assert_eq!(paused.state, JobState::Paused);
    assert!(
        paused.counts.done > 0 && paused.counts.done < 15,
        "{:?}",
        paused.counts
    );
    assert_eq!(
        paused.counts.pending,
        paused.counts.files - paused.counts.done
    );

    *s.transport.per_file.lock().unwrap() = Duration::ZERO;
    s.engine.resume(&job).await.unwrap();
    let state = tokio::time::timeout(Duration::from_secs(20), s.engine.wait(&job))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(state, JobState::Completed);
    assert_eq!(dest_files(&s.dest).len(), 15);
}

#[tokio::test]
async fn resumes_unfinished_jobs_on_startup() {
    let s = setup().await;
    let job = ready_job(&s, SendOptions::default()).await;
    // Simulate a crash mid-transfer: the job is stored as Transferring, nothing runs it.
    s.store
        .set_job_state(&job, "transferring", None)
        .await
        .unwrap();

    let engine = engine_with(s.store.clone(), s.transport.clone(), s.integration.clone());
    let resumed = engine.resume_unfinished().await.unwrap();
    assert_eq!(resumed, vec![job.clone()]);
    let state = tokio::time::timeout(Duration::from_secs(20), engine.wait(&job))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(state, JobState::Completed);
    assert_eq!(dest_files(&s.dest).len(), 3);
}

#[tokio::test]
async fn integration_problems_are_warnings() {
    let s = setup().await;
    *s.integration.problem.lock().unwrap() = Some(Problem {
        code: "fake.incompatible-encoding".into(),
        message: "Entry \"x\" will not be accessible".into(),
        path: Some("Acme/x".into()),
        retryable: false,
        fix: None,
    });
    let job = ready_job(&s, SendOptions::default()).await;
    assert_eq!(run_to_end(&s, &job).await, JobState::CompletedWithWarnings);
}

#[tokio::test]
async fn cancel_is_final() {
    let s = setup().await;
    let job = ready_job(&s, SendOptions::default()).await;
    s.engine.cancel(&job).await.unwrap();
    assert_eq!(s.engine.job(&job).await.unwrap().state, JobState::Cancelled);
    assert!(s.engine.start(&job).await.is_err());
}

#[tokio::test]
async fn files_finished_mid_batch_are_kept_when_paused() {
    let s = setup().await;
    for i in 0..20 {
        std::fs::write(s.src.join(format!("f{i:02}.txt")), b"x").unwrap();
    }
    // One transport call for all 23 files; pause while it is half way.
    let engine = engine_with_batch(
        s.store.clone(),
        s.transport.clone(),
        s.integration.clone(),
        100,
    );
    *s.transport.per_file.lock().unwrap() = Duration::from_millis(40);
    let job = engine
        .create_job(vec![s.src.clone()], &s.profile_id, SendOptions::default())
        .await
        .unwrap();
    engine.analyse(&job).await.unwrap();
    engine.start(&job).await.unwrap();
    tokio::time::sleep(Duration::from_millis(400)).await;
    engine.pause(&job).await.unwrap();
    let paused = engine.job(&job).await.unwrap();
    assert!(
        paused.counts.done >= 3 && paused.counts.done < 23,
        "files already sent must stay done: {:?}",
        paused.counts
    );

    *s.transport.per_file.lock().unwrap() = Duration::ZERO;
    let calls = s.transport.calls.load(Ordering::SeqCst);
    engine.resume(&job).await.unwrap();
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(20), engine.wait(&job))
            .await
            .unwrap()
            .unwrap(),
        JobState::Completed
    );
    assert_eq!(s.transport.calls.load(Ordering::SeqCst) - calls, 1);
    assert_eq!(engine.job(&job).await.unwrap().counts.done, 23);
}

#[tokio::test]
async fn unverifiable_destinations_warn_without_resending() {
    let s = setup().await;
    s.transport.no_hash.store(true, Ordering::SeqCst);
    let job = ready_job(
        &s,
        SendOptions {
            verify: true,
            ..SendOptions::default()
        },
    )
    .await;
    let calls = s.transport.calls.load(Ordering::SeqCst);
    assert_eq!(run_to_end(&s, &job).await, JobState::CompletedWithWarnings);
    assert_eq!(
        s.transport.calls.load(Ordering::SeqCst) - calls,
        1,
        "nothing resent"
    );
    let summary = s.engine.job(&job).await.unwrap();
    assert!(summary.error.unwrap().contains("could not be verified"));
    assert_eq!(summary.counts.mismatch, 0);
}

#[tokio::test]
async fn options_change_until_the_job_is_sent() {
    let s = setup().await;
    let job = s
        .engine
        .create_job(vec![s.src.clone()], &s.profile_id, SendOptions::default())
        .await
        .unwrap();
    let verify = SendOptions {
        verify: true,
        concurrency: 2,
        bwlimit: Some("20M".into()),
    };
    // Draft.
    s.engine.set_options(&job, verify.clone()).await.unwrap();
    assert_eq!(s.engine.job(&job).await.unwrap().options, verify);

    // Ready: the analysis is kept.
    assert_eq!(s.engine.analyse(&job).await.unwrap().state, JobState::Ready);
    s.engine
        .set_options(&job, SendOptions::default())
        .await
        .unwrap();
    let summary = s.engine.job(&job).await.unwrap();
    assert_eq!(summary.state, JobState::Ready);
    assert_eq!(summary.options, SendOptions::default());
    assert_eq!(summary.counts.files, 3);
    s.engine.set_options(&job, verify.clone()).await.unwrap();

    // Sent: refused, and the job keeps what it was sent with.
    assert_eq!(run_to_end(&s, &job).await, JobState::Completed);
    assert!(matches!(
        s.engine.set_options(&job, SendOptions::default()).await,
        Err(CoreError::Invalid(_))
    ));
    let summary = s.engine.job(&job).await.unwrap();
    assert_eq!(summary.options, verify);
    assert_eq!(summary.counts.verified_ok, 3);
    assert!(matches!(
        s.engine.set_options("nope", SendOptions::default()).await,
        Err(CoreError::Store(StoreError::NotFound(_)))
    ));
}

#[tokio::test]
async fn options_change_while_awaiting_approval() {
    let s = setup().await;
    std::fs::write(s.src.join("Fotos/Ver\u{e3}o.jpg"), b"other summer").unwrap();
    let job = s
        .engine
        .create_job(vec![s.src.clone()], &s.profile_id, SendOptions::default())
        .await
        .unwrap();
    s.engine.analyse(&job).await.unwrap();
    let verify = SendOptions {
        verify: true,
        ..SendOptions::default()
    };
    s.engine.set_options(&job, verify.clone()).await.unwrap();
    let summary = s.engine.job(&job).await.unwrap();
    assert_eq!(summary.state, JobState::AwaitingApproval);
    assert_eq!(summary.options, verify);
    assert_eq!(s.store.pending_issue_count(&job).await.unwrap(), 1);
}

#[tokio::test]
async fn decide_all_applies_one_decision_to_every_pending_issue() {
    let s = setup().await;
    // Three files that all become "Verão.jpg": two collisions to decide.
    std::fs::write(s.src.join("Fotos/Ver\u{e3}o.jpg"), b"nfc").unwrap();
    std::fs::write(s.src.join("Fotos/Vera\u{303}o.JPG"), b"upper").unwrap();
    let mut profile = s.engine.get_profile(&s.profile_id).await.unwrap();
    profile.target.case_sensitive = false;
    s.engine.save_profile(profile).await.unwrap();

    let job = s
        .engine
        .create_job(vec![s.src.clone()], &s.profile_id, SendOptions::default())
        .await
        .unwrap();
    assert!(
        matches!(
            s.engine.decide_all(&job, None, DecisionInput::Accept).await,
            Err(CoreError::Invalid(_))
        ),
        "no decisions in Draft"
    );
    let summary = s.engine.analyse(&job).await.unwrap();
    assert_eq!(summary.state, JobState::AwaitingApproval);
    assert_eq!(summary.pending_decisions, 2, "{summary:?}");

    assert!(matches!(
        s.engine
            .decide_all(&job, None, DecisionInput::Edit("x".into()))
            .await,
        Err(CoreError::Invalid(_))
    ));
    // Another rule's issues are not pending, so nothing changes.
    let n = s
        .engine
        .decide_all(&job, Some("nfc".into()), DecisionInput::Skip)
        .await
        .unwrap();
    assert_eq!(n, 0);
    assert_eq!(
        s.engine.job(&job).await.unwrap().state,
        JobState::AwaitingApproval
    );

    // One decided by hand keeps its decision.
    let issues = s
        .engine
        .issues(&job, Some("collision".into()), None, 10)
        .await
        .unwrap();
    s.engine
        .decide_issue(&job, issues[0].id, DecisionInput::Skip)
        .await
        .unwrap();
    assert_eq!(
        s.engine.job(&job).await.unwrap().state,
        JobState::AwaitingApproval
    );
    let n = s
        .engine
        .decide_all(&job, Some("collision".into()), DecisionInput::Accept)
        .await
        .unwrap();
    assert_eq!(n, 1);
    assert_eq!(s.engine.job(&job).await.unwrap().state, JobState::Ready);
    let decisions: Vec<_> = s
        .engine
        .issues(&job, Some("collision".into()), None, 10)
        .await
        .unwrap()
        .into_iter()
        .map(|i| i.decision)
        .collect();
    assert_eq!(decisions, ["skipped", "accepted"]);

    // Ready stays Ready; nothing is pending any more.
    let n = s
        .engine
        .decide_all(&job, None, DecisionInput::KeepOriginal)
        .await
        .unwrap();
    assert_eq!(n, 0);
    assert_eq!(s.engine.job(&job).await.unwrap().state, JobState::Ready);

    assert_eq!(run_to_end(&s, &job).await, JobState::Completed);
    assert!(matches!(
        s.engine.decide_all(&job, None, DecisionInput::Accept).await,
        Err(CoreError::Invalid(_))
    ));
}
