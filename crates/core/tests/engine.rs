//! End-to-end engine tests with the shared local-disk fake transport and a recording fake
//! integration.

use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use filebridge_core::backoff::Backoff;
use filebridge_core::{
    AppSettings, CoreError, DecisionInput, Engine, EngineOptions, EntryFilter, EntryStatus, Event,
    FileCategory, IssueDecision, JobState, LocalClock, LogLevel, Profile, Registry, SendOptions,
    SendWindow, Store, StoreError, TargetSpec,
};
use filebridge_integration_api::{
    AfterTransferReport, Integration, IntegrationEvent, IntegrationSink, JobContext, NameRule,
    Problem, StepProgress,
};
use filebridge_transport_api::{Backend, Destination, PreflightReport, RemotePath, TransportCaps};
use filebridge_transport_fake::FakeTransport;

#[derive(Default)]
struct FakeIntegration {
    changed: Mutex<Vec<String>>,
    problem: Mutex<Option<Problem>>,
    /// `after_transfer` never returns, as if the app died during stage 6.
    hang: std::sync::atomic::AtomicBool,
}

#[async_trait]
impl Integration for FakeIntegration {
    fn id(&self) -> &str {
        "fakeint"
    }

    fn required_caps(&self) -> TransportCaps {
        TransportCaps::empty()
    }

    fn name_rules(&self, _settings: &serde_json::Value) -> Vec<Box<dyn NameRule>> {
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
        if self.hang.load(Ordering::SeqCst) {
            std::future::pending::<()>().await;
        }
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
            local_clock: LocalClock::fixed(|| NOON),
        },
    )
}

/// The tests' local time: send windows are set around it.
const NOON: u32 = 12 * 3600;

fn window(start_hour: u16, end_hour: u16) -> AppSettings {
    AppSettings {
        bandwidth_limit: None,
        send_window: Some(SendWindow {
            start_minute: start_hour * 60,
            end_minute: end_hour * 60,
        }),
    }
}

/// A window that is closed at noon.
fn closed_window() -> AppSettings {
    window(22, 7)
}

async fn wait_for_state(engine: &Engine, job: &str, want: JobState) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let state = engine.job(job).await.unwrap().state;
        if state == want {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "job is {state:?}, never {want:?}"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
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
async fn the_queue_names_files_in_flight_and_the_ones_that_arrived() {
    let s = setup().await;
    let mut events = s.engine.subscribe();
    let job = ready_job(&s, SendOptions::default()).await;
    assert_eq!(run_to_end(&s, &job).await, JobState::Completed);

    let (mut in_flight, mut arrived, mut more) = (Vec::new(), Vec::new(), 0);
    while let Ok(e) = events.try_recv() {
        match e {
            Event::TransferProgress { in_flight: f, .. } => in_flight.extend(f),
            Event::EntriesDone {
                entries, more: m, ..
            } => {
                arrived.extend(entries);
                more += m;
            }
            _ => {}
        }
    }
    // The fake sends one file at a time and names each one while it is in flight.
    assert!(!in_flight.is_empty());
    assert!(in_flight.iter().all(|f| f.idx.is_some()), "{in_flight:?}");
    let summer = in_flight
        .iter()
        .find(|f| f.name == "Ver\u{e3}o.jpg")
        .expect("the renamed photo was in flight");
    assert_eq!(summer.dir, "Acme/Fotos");
    assert_eq!(summer.category, FileCategory::Image);
    assert_eq!(summer.renamed_from.as_deref(), Some("Vera\u{303}o.jpg"));
    let winter = in_flight.iter().find(|f| f.name == "Inverno.jpg").unwrap();
    assert_eq!(
        winter.renamed_from, None,
        "unchanged names carry no source name"
    );

    let mut names: Vec<_> = arrived.iter().map(|d| d.name.as_str()).collect();
    names.sort_unstable();
    assert_eq!(
        names,
        ["Inverno.jpg", "Relat\u{f3}rio.pdf", "Ver\u{e3}o.jpg"]
    );
    assert_eq!(more, 0);
}

#[tokio::test]
async fn a_job_says_what_it_contains_and_where_it_came_from() {
    let s = setup().await;
    let job = ready_job(&s, SendOptions::default()).await;
    let cats = s.engine.job_categories(&job).await.unwrap();
    let by: Vec<_> = cats
        .iter()
        .map(|c| (c.category, c.files, c.bytes))
        .collect();
    assert_eq!(
        by,
        [(FileCategory::Image, 2, 12), (FileCategory::Document, 1, 6)]
    );

    // Only sent jobs count as recent sources.
    assert!(s.engine.recent_sources(10).await.unwrap().is_empty());
    assert_eq!(run_to_end(&s, &job).await, JobState::Completed);
    let recent = s.engine.recent_sources(10).await.unwrap();
    assert_eq!(recent.len(), 1);
    assert_eq!(recent[0].path, s.src.display().to_string());
    assert!(recent[0].exists);
    assert_eq!(recent[0].last_job_id, job);

    std::fs::rename(&s.src, s.src.with_file_name("Renamed")).unwrap();
    assert!(!s.engine.recent_sources(10).await.unwrap()[0].exists);
}

#[tokio::test]
async fn the_files_of_a_transfer_prove_what_arrived() {
    use sha2::{Digest as _, Sha256};
    let s = setup().await;
    s.transport
        .fatal_names
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
    assert_eq!(run_to_end(&s, &job).await, JobState::CompletedWithWarnings);

    let all = s
        .engine
        .files_page(&job, EntryFilter::default(), None, 100)
        .await
        .unwrap();
    assert_eq!(all.items.len(), 3, "files only by default");
    assert!(all.next.is_none());
    let summer = all
        .items
        .iter()
        .find(|e| e.name == "Ver\u{e3}o.jpg")
        .unwrap();
    assert_eq!(summer.status, EntryStatus::Verified);
    assert_eq!(summer.dir, "Acme/Fotos");
    assert_eq!(summer.source_name.as_deref(), Some("Vera\u{303}o.jpg"));
    assert_eq!(summer.category, FileCategory::Image);

    let problems = s
        .engine
        .files_page(
            &job,
            EntryFilter {
                statuses: EntryStatus::ALL
                    .into_iter()
                    .filter(|s| s.is_problem())
                    .collect(),
                ..EntryFilter::default()
            },
            None,
            100,
        )
        .await
        .unwrap();
    assert_eq!(problems.items.len(), 1);
    assert_eq!(problems.items[0].name, "Inverno.jpg");
    assert_eq!(problems.items[0].status, EntryStatus::Failed);

    let facets = s.engine.entry_facets(&job).await.unwrap();
    assert_eq!((facets.problems, facets.renamed), (1, 2));

    // The detail keeps the source's SHA-256, as verification computed it.
    let detail = s.engine.entry_detail(&job, summer.idx).await.unwrap();
    let expected: String = Sha256::digest(b"summer")
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    let hash = detail.hash.expect("hash kept by verify");
    assert_eq!((hash.algo.as_str(), hash.hex), ("sha256", expected));
    assert!(detail.source_path.ends_with("Acme/Fotos/Vera\u{303}o.jpg"));
    assert!(
        detail
            .dest_path
            .unwrap()
            .ends_with("/Acme/Fotos/Ver\u{e3}o.jpg")
    );
    assert_eq!(detail.issues.len(), 1);
    assert_eq!(detail.issues[0].rule, "nfc");
    let failed = s
        .engine
        .entry_detail(&job, problems.items[0].idx)
        .await
        .unwrap();
    assert_eq!(failed.error.as_deref(), Some("permission denied"));
    assert!(failed.hash.is_none(), "never verified");
}

#[tokio::test]
async fn a_destinations_space_is_kept_from_its_last_check() {
    let s = setup().await;
    let never = s
        .engine
        .profile_storage(&s.profile_id, false)
        .await
        .unwrap();
    assert_eq!((never.checked_at, never.free_bytes), (None, None));

    s.engine.test_profile(&s.profile_id).await.unwrap();
    let kept = s
        .engine
        .profile_storage(&s.profile_id, false)
        .await
        .unwrap();
    assert_eq!(
        (kept.free_bytes, kept.total_bytes, kept.used_bytes),
        (Some(1 << 40), Some(3 << 40), Some(2 << 40))
    );
    assert!(!kept.stale && !kept.low);
    assert!(kept.last_error.is_none());

    // A send measures the destination again when its transfer ends.
    let job = ready_job(&s, SendOptions::default()).await;
    assert_eq!(run_to_end(&s, &job).await, JobState::Completed);
    let after = s
        .engine
        .profile_storage(&s.profile_id, false)
        .await
        .unwrap();
    assert!(after.checked_at.unwrap() >= kept.checked_at.unwrap());
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
async fn the_job_log_tells_what_happened_while_no_one_watched() {
    let s = setup().await;
    s.transport.fail_calls.store(1, Ordering::SeqCst);
    let job = ready_job(
        &s,
        SendOptions {
            verify: true,
            ..SendOptions::default()
        },
    )
    .await;
    assert_eq!(run_to_end(&s, &job).await, JobState::Completed);

    let oldest_first = s.engine.job_log(&job, Some(0), None, 500).await.unwrap();
    let kinds: Vec<_> = oldest_first.iter().map(|e| e.kind.as_str()).collect();
    let pos = |k: &str| {
        kinds
            .iter()
            .position(|x| *x == k)
            .unwrap_or_else(|| panic!("no {k} in {kinds:?}"))
    };
    assert_eq!(kinds[0], "created");
    assert!(pos("analysis") < pos("retry"));
    assert!(pos("retry") < pos("transfer_round"));
    assert!(pos("transfer_round") < pos("verify"));
    assert_eq!(*kinds.last().unwrap(), "finished");

    let retry = &oldest_first[pos("retry")];
    assert_eq!(retry.level, LogLevel::Warn);
    assert_eq!(retry.data["attempt"], 1);
    let round = &oldest_first[pos("transfer_round")];
    assert_eq!(
        (round.data["sent"].as_u64(), round.data["failed"].as_u64()),
        (Some(3), Some(0))
    );
    let verify = &oldest_first[pos("verify")];
    assert_eq!(verify.data["checked"], 3);
    assert_eq!(verify.level, LogLevel::Info);
    assert_eq!(oldest_first.last().unwrap().data["state"], "completed");

    // Newest first, a page at a time, gives the same log backwards.
    let newest = s.engine.job_log(&job, None, None, 2).await.unwrap();
    assert_eq!(newest[0].id, oldest_first.last().unwrap().id);
    let older = s
        .engine
        .job_log(&job, None, Some(newest[1].id), 500)
        .await
        .unwrap();
    assert_eq!(newest.len() + older.len(), oldest_first.len());

    // A profile change is app-level: it is in no job's log.
    assert!(kinds.iter().all(|k| *k != "profile"));
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
        .change_job_state(
            &job,
            filebridge_store::StateChange {
                from: "ready".into(),
                to: "transferring".into(),
                ..Default::default()
            },
        )
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
    assert_eq!(decisions, [IssueDecision::Skipped, IssueDecision::Accepted]);

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

#[tokio::test]
async fn an_issue_is_decided_only_through_its_own_job() {
    let s = setup().await;
    std::fs::write(s.src.join("Fotos/Ver\u{e3}o.jpg"), b"other summer").unwrap();
    let mut jobs = Vec::new();
    for _ in 0..2 {
        let job = s
            .engine
            .create_job(vec![s.src.clone()], &s.profile_id, SendOptions::default())
            .await
            .unwrap();
        s.engine.analyse(&job).await.unwrap();
        jobs.push(job);
    }
    let theirs = s
        .engine
        .issues(&jobs[0], Some("collision".into()), None, 1)
        .await
        .unwrap();
    assert!(matches!(
        s.engine
            .decide_issue(&jobs[1], theirs[0].id, DecisionInput::Skip)
            .await,
        Err(CoreError::Store(StoreError::NotFound(_)))
    ));
    assert_eq!(s.store.pending_issue_count(&jobs[0]).await.unwrap(), 1);
}

#[tokio::test]
async fn edited_names_are_checked_when_decided() {
    let s = setup().await;
    std::fs::write(s.src.join("Fotos/Ver\u{e3}o.jpg"), b"other summer").unwrap();
    let job = s
        .engine
        .create_job(vec![s.src.clone()], &s.profile_id, SendOptions::default())
        .await
        .unwrap();
    s.engine.analyse(&job).await.unwrap();
    let issue = s
        .engine
        .issues(&job, Some("collision".into()), None, 1)
        .await
        .unwrap()
        .remove(0);
    for bad in ["a/b", "..", "Vera\u{303}o de 2024.jpg"] {
        assert!(
            matches!(
                s.engine
                    .decide_issue(&job, issue.id, DecisionInput::Edit(bad.into()))
                    .await,
                Err(CoreError::Invalid(_))
            ),
            "{bad:?} is refused when decided"
        );
    }
    assert_eq!(
        s.engine.job(&job).await.unwrap().state,
        JobState::AwaitingApproval
    );
    s.engine
        .decide_issue(
            &job,
            issue.id,
            DecisionInput::Edit("Verão de 2024.jpg".into()),
        )
        .await
        .unwrap();
    assert_eq!(run_to_end(&s, &job).await, JobState::Completed);
    assert!(dest_files(&s.dest).contains(&"Acme/Fotos/Verão de 2024.jpg".to_owned()));
}

#[tokio::test]
async fn keeping_the_original_skips_only_that_rule() {
    let s = setup().await;
    let job = ready_job(&s, SendOptions::default()).await;
    let nfc = s
        .engine
        .issues(&job, Some("nfc".into()), None, 10)
        .await
        .unwrap();
    let report = nfc
        .iter()
        .find(|i| i.path.ends_with(".pdf"))
        .expect("the report has an nfc issue");
    s.engine
        .decide_issue(&job, report.id, DecisionInput::KeepOriginal)
        .await
        .unwrap();
    assert_eq!(run_to_end(&s, &job).await, JobState::Completed);
    let files = dest_files(&s.dest);
    assert!(
        files.contains(&"Acme/Relato\u{301}rio.pdf".to_owned()),
        "{files:?}"
    );
    assert!(files.contains(&"Acme/Fotos/Verão.jpg".to_owned()));
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn an_undecodable_name_can_only_be_edited_or_skipped() {
    use std::os::unix::ffi::OsStrExt;
    let s = setup().await;
    std::fs::write(
        s.src
            .join(std::ffi::OsStr::from_bytes(b"Patol\xf3gica.pdf")),
        b"latin-1",
    )
    .unwrap();
    let job = s
        .engine
        .create_job(vec![s.src.clone()], &s.profile_id, SendOptions::default())
        .await
        .unwrap();
    s.engine.analyse(&job).await.unwrap();
    for input in [DecisionInput::Accept, DecisionInput::KeepOriginal] {
        assert!(matches!(
            s.engine
                .decide_all(&job, Some("encoding".into()), input)
                .await,
            Err(CoreError::Invalid(_))
        ));
    }
    assert_eq!(s.store.pending_issue_count(&job).await.unwrap(), 1);
    let n = s
        .engine
        .decide_all(&job, Some("encoding".into()), DecisionInput::Skip)
        .await
        .unwrap();
    assert_eq!(n, 1);
    assert_eq!(run_to_end(&s, &job).await, JobState::Completed);
    assert_eq!(dest_files(&s.dest).len(), 3);
}

#[tokio::test]
async fn warnings_of_earlier_stages_survive_a_restart() {
    let s = setup().await;
    s.transport.no_hash.store(true, Ordering::SeqCst);
    s.integration.hang.store(true, Ordering::SeqCst);
    let job = ready_job(
        &s,
        SendOptions {
            verify: true,
            ..SendOptions::default()
        },
    )
    .await;
    s.engine.start(&job).await.unwrap();
    // Verification can't hash anything (a warning), then stage 6 hangs: the app "dies" there.
    tokio::time::timeout(Duration::from_secs(20), async {
        while s.engine.job(&job).await.unwrap().state != JobState::Integrating {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();

    s.integration.hang.store(false, Ordering::SeqCst);
    let engine = engine_with(s.store.clone(), s.transport.clone(), s.integration.clone());
    assert_eq!(engine.resume_unfinished().await.unwrap(), vec![job.clone()]);
    let state = tokio::time::timeout(Duration::from_secs(20), engine.wait(&job))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(state, JobState::CompletedWithWarnings);
    let error = engine.job(&job).await.unwrap().error.unwrap_or_default();
    assert!(error.contains("could not be verified"), "{error}");
}

#[tokio::test]
async fn a_file_missing_at_the_destination_is_sent_again() {
    let s = setup().await;
    s.transport
        .lose_once
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
    let calls = s.transport.calls.load(Ordering::SeqCst);
    assert_eq!(run_to_end(&s, &job).await, JobState::Completed);
    assert!(dest_files(&s.dest).contains(&"Acme/Fotos/Inverno.jpg".to_owned()));
    assert_eq!(
        s.transport.calls.load(Ordering::SeqCst) - calls,
        2,
        "one more call for the missing file"
    );
    assert_eq!(s.engine.job(&job).await.unwrap().counts.verified_ok, 3);
}

// ---- settings and the send window ----------------------------------------------------------

#[tokio::test]
async fn settings_are_validated_saved_and_applied() {
    let s = setup().await;
    assert_eq!(s.engine.get_settings(), AppSettings::default());
    let bad = AppSettings {
        send_window: Some(SendWindow {
            start_minute: 60,
            end_minute: 60,
        }),
        ..Default::default()
    };
    assert!(matches!(
        s.engine.save_settings(bad).await,
        Err(CoreError::Invalid(_))
    ));
    let limited = AppSettings {
        bandwidth_limit: Some(2_500_000),
        ..window(22, 7)
    };
    s.engine.save_settings(limited.clone()).await.unwrap();
    s.engine.save_settings(window(22, 7)).await.unwrap();
    assert_eq!(
        *s.transport.bandwidth_limits.lock().unwrap(),
        [Some(2_500_000), None]
    );

    // A restart reads them back and applies the limit to the fresh transport.
    s.engine.save_settings(limited.clone()).await.unwrap();
    let transport = Arc::new(FakeTransport::default());
    let engine = engine_with(s.store.clone(), transport.clone(), s.integration.clone());
    assert_eq!(engine.load_settings().await.unwrap(), limited);
    assert_eq!(engine.get_settings(), limited);
    assert_eq!(
        *transport.bandwidth_limits.lock().unwrap(),
        [Some(2_500_000)]
    );
}

#[tokio::test]
async fn a_closed_window_holds_the_transfer_until_it_opens() {
    let s = setup().await;
    s.engine.save_settings(closed_window()).await.unwrap();
    let mut events = s.engine.subscribe();
    let job = ready_job(&s, SendOptions::default()).await;
    s.engine.start(&job).await.unwrap();
    wait_for_state(&s.engine, &job, JobState::Waiting).await;
    let opens_at = loop {
        if let Event::SendWindowWait { opens_at, .. } = events.recv().await.unwrap() {
            break opens_at;
        }
    };
    // 22:00 is 10 hours after the tests' noon.
    let expected = filebridge_store::now_ms() + 10 * 3600 * 1000;
    assert!(
        (opens_at - expected).abs() < 60_000,
        "{opens_at} vs {expected}"
    );
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(s.transport.calls.load(Ordering::SeqCst), 0);
    assert!(dest_files(&s.dest).is_empty());

    // Opening the window (here: a window that includes noon) lets it go at once.
    s.engine.save_settings(window(11, 13)).await.unwrap();
    let state = tokio::time::timeout(Duration::from_secs(20), s.engine.wait(&job))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(state, JobState::Completed);
    assert_eq!(dest_files(&s.dest).len(), 3);
}

#[tokio::test]
async fn a_window_closing_mid_transfer_keeps_what_arrived() {
    let s = setup().await;
    for i in 0..12 {
        std::fs::write(s.src.join(format!("f{i:02}.txt")), b"x").unwrap();
    }
    *s.transport.per_file.lock().unwrap() = Duration::from_millis(30);
    let job = ready_job(&s, SendOptions::default()).await;
    s.engine.start(&job).await.unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;
    s.engine.save_settings(closed_window()).await.unwrap();
    wait_for_state(&s.engine, &job, JobState::Waiting).await;
    let held = s.engine.job(&job).await.unwrap();
    assert!(
        held.counts.done > 0 && held.counts.done < 15,
        "{:?}",
        held.counts
    );
    assert_eq!(held.counts.pending, held.counts.files - held.counts.done);

    *s.transport.per_file.lock().unwrap() = Duration::ZERO;
    s.engine
        .save_settings(AppSettings::default())
        .await
        .unwrap();
    let state = tokio::time::timeout(Duration::from_secs(20), s.engine.wait(&job))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(state, JobState::Completed);
    assert_eq!(dest_files(&s.dest).len(), 15);
}

#[tokio::test]
async fn a_waiting_job_can_be_paused_resumed_and_cancelled() {
    let s = setup().await;
    s.engine.save_settings(closed_window()).await.unwrap();
    let job = ready_job(&s, SendOptions::default()).await;
    s.engine.start(&job).await.unwrap();
    wait_for_state(&s.engine, &job, JobState::Waiting).await;
    s.engine.pause(&job).await.unwrap();
    assert_eq!(s.engine.job(&job).await.unwrap().state, JobState::Paused);
    s.engine.resume(&job).await.unwrap();
    wait_for_state(&s.engine, &job, JobState::Waiting).await;
    s.engine.cancel(&job).await.unwrap();
    assert_eq!(s.engine.job(&job).await.unwrap().state, JobState::Cancelled);
    assert_eq!(s.transport.calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn a_waiting_job_keeps_waiting_after_a_restart() {
    let s = setup().await;
    s.engine.save_settings(closed_window()).await.unwrap();
    let job = ready_job(&s, SendOptions::default()).await;
    s.engine.start(&job).await.unwrap();
    wait_for_state(&s.engine, &job, JobState::Waiting).await;
    // The app stops while the job waits.
    s.engine.pause(&job).await.unwrap();
    s.store
        .change_job_state(
            &job,
            filebridge_store::StateChange {
                from: "paused".into(),
                to: "waiting".into(),
                ..Default::default()
            },
        )
        .await
        .unwrap();

    let engine = engine_with(s.store.clone(), s.transport.clone(), s.integration.clone());
    engine.load_settings().await.unwrap();
    assert_eq!(engine.resume_unfinished().await.unwrap(), vec![job.clone()]);
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(engine.job(&job).await.unwrap().state, JobState::Waiting);
    assert_eq!(s.transport.calls.load(Ordering::SeqCst), 0);

    engine.save_settings(AppSettings::default()).await.unwrap();
    let state = tokio::time::timeout(Duration::from_secs(20), engine.wait(&job))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(state, JobState::Completed);
}

#[tokio::test]
async fn verification_and_integration_run_outside_the_window() {
    let s = setup().await;
    s.integration.hang.store(true, Ordering::SeqCst);
    let job = ready_job(
        &s,
        SendOptions {
            verify: true,
            ..Default::default()
        },
    )
    .await;
    s.engine.start(&job).await.unwrap();
    wait_for_state(&s.engine, &job, JobState::Integrating).await;
    // Stopped after the transfer; restarted in verification with the window closed.
    s.engine.pause(&job).await.unwrap();
    s.store
        .change_job_state(
            &job,
            filebridge_store::StateChange {
                from: "paused".into(),
                to: "verifying".into(),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    s.integration.hang.store(false, Ordering::SeqCst);
    let calls = s.transport.calls.load(Ordering::SeqCst);

    let engine = engine_with(s.store.clone(), s.transport.clone(), s.integration.clone());
    engine.save_settings(closed_window()).await.unwrap();
    engine.resume_unfinished().await.unwrap();
    let state = tokio::time::timeout(Duration::from_secs(20), engine.wait(&job))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(state, JobState::Completed);
    assert_eq!(
        s.transport.calls.load(Ordering::SeqCst),
        calls,
        "nothing resent"
    );
    assert_eq!(engine.job(&job).await.unwrap().counts.verified_ok, 3);
}
