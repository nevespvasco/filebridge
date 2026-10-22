//! End-to-end engine tests with the shared local-disk fake transport and a recording fake
//! integration.

use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use filebridge_core::backoff::Backoff;
use filebridge_core::{
    AppSettings, BrowseTarget, CoreError, DecisionInput, Engine, EngineOptions, EntryFilter,
    EntryStatus, Event, FileCategory, IssueDecision, JobFilter, JobGroup, JobKind, JobState,
    LocalClock, LogLevel, Profile, ReclaimBlocker, Registry, ReportFormat, ReportRequest,
    ReportScope, SendOptions, SendWindow, SourceProblem, Store, StoreError, TargetSpec,
};
use filebridge_integration_api::{
    AfterTransferReport, Integration, IntegrationEvent, IntegrationSink, JobContext, NameRule,
    Problem, StepProgress,
};
use filebridge_transport_api::{
    Backend, Destination, PreflightReport, RemotePath, TransportCaps, TransportError,
};
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

    /// `{"needs_password": true}`: like Nextcloud's WebDAV mode, the profile can't be without one.
    fn check_profile(
        &self,
        destination: &Destination,
        settings: &serde_json::Value,
    ) -> filebridge_integration_api::Result<()> {
        let has = matches!(
            &destination.backend,
            Backend::Sftp {
                password: Some(_),
                ..
            } | Backend::Webdav {
                password: Some(_),
                ..
            }
        );
        if settings["needs_password"] == true && !has {
            return Err(filebridge_integration_api::IntegrationError::fatal(
                "this destination needs its password",
            ));
        }
        Ok(())
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
async fn transfers_are_named_found_and_sent_again_with_their_decisions() {
    let s = setup().await;
    let job = ready_job(&s, SendOptions::default()).await;
    s.engine
        .set_label(&job, Some("  Fotos de Verão  ".into()))
        .await
        .unwrap();
    // Keep one name as it is: the repeat must not send it under the corrected name.
    let issues = s.engine.issues(&job, None, None, 10).await.unwrap();
    let summer = issues
        .iter()
        .find(|i| i.path.contains("Vera"))
        .expect("the NFD photo");
    s.engine
        .decide_issue(&job, summer.id, DecisionInput::KeepOriginal)
        .await
        .unwrap();
    assert!(
        s.engine.send_again(&job, None).await.is_err(),
        "only a finished job"
    );
    assert_eq!(run_to_end(&s, &job).await, JobState::Completed);
    s.engine
        .set_label(&job, Some("Fotos de Verão".into()))
        .await
        .unwrap();
    assert!(
        s.engine
            .set_label(&job, Some("x".repeat(121)))
            .await
            .is_err()
    );

    let other = s
        .engine
        .create_job(
            vec![s.src.join("Fotos")],
            &s.profile_id,
            SendOptions::default(),
        )
        .await
        .unwrap();

    let page = |filter: JobFilter| s.engine.list_jobs_page(filter, 50, 0);
    let all = page(JobFilter::default()).await.unwrap();
    assert_eq!(
        (all.total, all.items[0].id.as_str()),
        (2, other.as_str()),
        "newest first"
    );
    let finished = page(JobFilter {
        groups: vec![JobGroup::Finished],
        ..Default::default()
    })
    .await
    .unwrap();
    assert_eq!(finished.items.len(), 1);
    assert_eq!(finished.items[0].label.as_deref(), Some("Fotos de Verão"));
    let by_name = page(JobFilter {
        query: Some("verão".into()),
        ..Default::default()
    })
    .await
    .unwrap();
    assert_eq!(by_name.total, 1, "the label matches");
    let by_destination = page(JobFilter {
        query: Some("test".into()),
        ..Default::default()
    })
    .await
    .unwrap();
    assert_eq!(by_destination.total, 2, "the profile's name matches");
    let not_sent = page(JobFilter {
        groups: vec![JobGroup::NotSent],
        ..Default::default()
    })
    .await
    .unwrap();
    assert_eq!(not_sent.items[0].id, other);

    // Send again: same sources and name; the kept name is decided already.
    let again = s.engine.send_again(&job, None).await.unwrap();
    let summary = s.engine.analyse(&again).await.unwrap();
    assert_eq!(summary.state, JobState::Ready, "nothing left to decide");
    let repeated = s.engine.job(&again).await.unwrap();
    assert_eq!(repeated.label.as_deref(), Some("Fotos de Verão"));
    assert_eq!(repeated.again_of.as_deref(), Some(job.as_str()));
    let carried: Vec<_> = s
        .engine
        .issues(&again, None, None, 10)
        .await
        .unwrap()
        .into_iter()
        .filter(|i| i.carried_over)
        .collect();
    assert_eq!(carried.len(), 1);
    assert_eq!(carried[0].decision, IssueDecision::Rejected);
    assert_eq!(run_to_end(&s, &again).await, JobState::Completed);
    let files = dest_files(&s.dest);
    assert!(
        !files.contains(&"Acme/Fotos/Ver\u{e3}o.jpg".to_owned()),
        "never sent under the corrected name: {files:?}"
    );
}

#[tokio::test]
async fn issues_are_decided_by_selection_or_by_path() {
    let s = setup().await;
    // Two collisions, in two folders.
    std::fs::write(s.src.join("Fotos/Ver\u{e3}o.jpg"), b"other summer").unwrap();
    std::fs::create_dir_all(s.src.join("Contratos")).unwrap();
    std::fs::write(s.src.join("Contratos/Ata\u{301}.pdf"), b"a").unwrap();
    std::fs::write(s.src.join("Contratos/At\u{e1}.pdf"), b"b").unwrap();
    let job = s
        .engine
        .create_job(vec![s.src.clone()], &s.profile_id, SendOptions::default())
        .await
        .unwrap();
    assert_eq!(s.engine.analyse(&job).await.unwrap().pending_decisions, 2);

    let in_contratos = s
        .engine
        .rule_counts_matching(&job, Some("contratos".into()))
        .await
        .unwrap();
    let collisions = in_contratos.iter().find(|r| r.rule == "collision").unwrap();
    assert_eq!(collisions.pending, 1, "ASCII case doesn't matter");
    let n = s
        .engine
        .decide_all_matching(
            &job,
            Some("collision".into()),
            DecisionInput::Skip,
            Some("Contratos".into()),
        )
        .await
        .unwrap();
    assert_eq!(n, 1, "only the issue under Contratos");
    assert_eq!(
        s.engine.job(&job).await.unwrap().state,
        JobState::AwaitingApproval
    );

    // By selection: the other one.
    let left: Vec<i64> = s
        .engine
        .issues(&job, Some("collision".into()), None, 10)
        .await
        .unwrap()
        .into_iter()
        .filter(|i| i.decision == IssueDecision::Pending)
        .map(|i| i.id)
        .collect();
    assert_eq!(left.len(), 1);
    let done = s
        .engine
        .decide_many(&job, left, DecisionInput::Accept)
        .await
        .unwrap();
    assert_eq!((done.changed, done.refused.len()), (1, 0));
    assert_eq!(s.engine.job(&job).await.unwrap().state, JobState::Ready);
    assert!(
        s.engine
            .decide_many(&job, vec![1], DecisionInput::Edit("x".into()))
            .await
            .is_err(),
        "an edit is for one issue"
    );
}

#[tokio::test]
async fn a_finished_transfer_exports_its_proof() {
    let s = setup().await;
    let job = ready_job(
        &s,
        SendOptions {
            verify: true,
            ..SendOptions::default()
        },
    )
    .await;
    let req = |format, scope, path: &std::path::Path| ReportRequest {
        format,
        scope,
        include_dirs: false,
        path: path.display().to_string(),
    };
    let csv_path = s.dest.parent().unwrap().join("report.csv");
    assert!(
        s.engine
            .export_report(&job, req(ReportFormat::Csv, ReportScope::All, &csv_path))
            .await
            .is_err(),
        "only a finished transfer"
    );
    s.engine
        .set_label(&job, Some("Fotos, 2019".into()))
        .await
        .unwrap();
    assert_eq!(run_to_end(&s, &job).await, JobState::Completed);

    let done = s
        .engine
        .export_report(&job, req(ReportFormat::Csv, ReportScope::All, &csv_path))
        .await
        .unwrap();
    assert_eq!(done.rows, 3);
    let csv = std::fs::read_to_string(&csv_path).unwrap();
    assert!(csv.starts_with('\u{feff}'), "a BOM for spreadsheets");
    assert!(csv.contains("# transfer: Fotos, 2019"));
    let rows: Vec<&str> = csv
        .lines()
        .filter(|l| !l.starts_with('#') && !l.starts_with('\u{feff}'))
        .collect();
    assert!(rows[0].starts_with("source_path,"));
    assert_eq!(rows.len(), 4, "header + 3 files");
    let summer = rows
        .iter()
        .find(|l| l.contains("Vera\u{303}o.jpg"))
        .unwrap();
    assert!(summer.contains(",verified,nfc,"), "{summer}");
    assert!(!std::path::Path::new(&format!("{}.part", csv_path.display())).exists());

    let json_path = s.dest.parent().unwrap().join("renamed.json");
    let r = s
        .engine
        .export_report(
            &job,
            req(ReportFormat::Json, ReportScope::Renamed, &json_path),
        )
        .await
        .unwrap();
    assert_eq!(r.rows, 2, "the two NFD names");
    let v: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&json_path).unwrap()).unwrap();
    assert_eq!(v["format"], "filebridge-report/1");
    assert_eq!(v["entries"].as_array().unwrap().len(), 2);
    assert_eq!(v["entries"][0]["status"], "verified");
    assert_eq!(v["entries"][0]["sha256"].as_str().unwrap().len(), 64);
    assert_eq!(v["destination"]["backend"], "local");
    assert_eq!(v["destination"]["profile"], "Test");
}

/// Every file under `dir` with its bytes and modification time: to prove nothing changed.
fn snapshot(dir: &Path) -> Vec<(PathBuf, Vec<u8>, std::time::SystemTime)> {
    let mut out: Vec<_> = walk(dir)
        .into_iter()
        .map(|p| {
            let m = std::fs::metadata(&p).unwrap().modified().unwrap();
            let data = std::fs::read(&p).unwrap();
            (p, data, m)
        })
        .collect();
    out.sort();
    out
}

#[tokio::test]
async fn a_verified_unchanged_source_is_shown_as_reclaimable_never_freed() {
    let s = setup().await;
    let job = ready_job(
        &s,
        SendOptions {
            verify: true,
            ..SendOptions::default()
        },
    )
    .await;
    assert_eq!(run_to_end(&s, &job).await, JobState::Completed);
    let before = snapshot(&s.src);

    let status = s.engine.reclaim_status(&job).await.unwrap();
    assert!(!status.eligible);
    assert!(
        status
            .blockers
            .iter()
            .any(|b| b.code == ReclaimBlocker::NotChecked)
    );

    let status = s.engine.recheck_sources(&job).await.unwrap();
    assert!(status.eligible, "{status:?}");
    assert_eq!(status.bytes, 18);
    assert!(!status.size_only, "the scan kept modification times");
    let page = s
        .engine
        .list_jobs_page(
            JobFilter {
                reclaimable: true,
                ..Default::default()
            },
            10,
            0,
        )
        .await
        .unwrap();
    assert_eq!(page.items.len(), 1);

    s.engine.dismiss_reclaim(&job, true).await.unwrap();
    let hidden = s
        .engine
        .list_jobs_page(
            JobFilter {
                reclaimable: true,
                ..Default::default()
            },
            10,
            0,
        )
        .await
        .unwrap();
    assert!(hidden.items.is_empty(), "hidden from the total");
    assert!(s.engine.reclaim_status(&job).await.unwrap().dismissed);

    // Nothing at the source changed through any of that.
    assert_eq!(snapshot(&s.src), before);

    // A file changed after the send blocks it, and is named.
    std::fs::write(s.src.join("Fotos/Inverno.jpg"), b"winter, edited").unwrap();
    let status = s.engine.recheck_sources(&job).await.unwrap();
    assert!(!status.eligible);
    let changed = status.sources[0]
        .blockers
        .iter()
        .find(|b| b.code == ReclaimBlocker::Changed)
        .expect("a changed file");
    assert_eq!(changed.count, Some(1));
    assert_eq!(status.changed_sample, ["Acme/Fotos/Inverno.jpg"]);
}

#[tokio::test]
async fn an_unverified_transfer_is_never_reclaimable() {
    let s = setup().await;
    let job = ready_job(&s, SendOptions::default()).await;
    assert_eq!(run_to_end(&s, &job).await, JobState::Completed);
    let status = s.engine.recheck_sources(&job).await.unwrap();
    assert!(!status.eligible);
    assert!(
        status
            .blockers
            .iter()
            .any(|b| b.code == ReclaimBlocker::NotVerified)
    );
}

#[tokio::test]
async fn folders_are_browsed_checked_and_sent_into() {
    let s = setup().await;
    let target = || BrowseTarget::Profile {
        id: s.profile_id.clone(),
    };
    assert!(
        s.engine
            .browse(target(), None, None)
            .await
            .unwrap()
            .folders
            .is_empty()
    );

    // A folder name the destination would change is refused, with the fix.
    let nfd = s
        .engine
        .check_folder_name(target(), "Vera\u{303}o".into())
        .await
        .unwrap();
    assert!(!nfd.ok);
    assert_eq!(nfd.proposed.as_deref(), Some("Ver\u{e3}o"));
    assert!(
        s.engine
            .check_folder_name(target(), "2019".into())
            .await
            .unwrap()
            .ok
    );
    let refused = s
        .engine
        .create_job(
            vec![s.src.clone()],
            &s.profile_id,
            SendOptions {
                target_dir: Some("Vera\u{303}o".into()),
                ..SendOptions::default()
            },
        )
        .await;
    assert!(
        refused.is_err(),
        "a target folder the destination would rename"
    );

    // Sent into a folder that doesn't exist yet: the send creates it.
    let job = ready_job(
        &s,
        SendOptions {
            verify: true,
            target_dir: Some("2019/Novas".into()),
            ..SendOptions::default()
        },
    )
    .await;
    assert_eq!(run_to_end(&s, &job).await, JobState::Completed);
    assert!(dest_files(&s.dest).contains(&"2019/Novas/Acme/Fotos/Inverno.jpg".to_owned()));
    let changed = s.integration.changed.lock().unwrap().clone();
    assert!(
        changed.iter().all(|p| p.starts_with("2019/Novas/")),
        "{changed:?}"
    );

    let root = s.engine.browse(target(), None, None).await.unwrap();
    assert_eq!(
        root.folders
            .iter()
            .map(|f| f.name.as_str())
            .collect::<Vec<_>>(),
        ["2019"]
    );
    let inside = s
        .engine
        .browse(target(), Some("2019".into()), None)
        .await
        .unwrap();
    assert_eq!(inside.segments, ["2019"]);
    assert_eq!(inside.folders[0].path, "2019/Novas");
}

#[tokio::test]
async fn sources_are_checked_without_reading_them() {
    let s = setup().await;
    let root = s.src.parent().unwrap().to_path_buf();
    let big = root.join("big");
    std::fs::create_dir_all(&big).unwrap();
    for i in 0..1001 {
        std::fs::write(big.join(format!("{i}.txt")), b"").unwrap();
    }
    #[cfg(unix)]
    std::os::unix::fs::symlink(s.src.join("Fotos"), root.join("link")).unwrap();
    let before = snapshot(&s.src);
    let p = |x: &Path| x.display().to_string();

    let got = s
        .engine
        .inspect_sources(vec![
            p(&s.src),
            p(&s.src.join("Fotos")),
            p(&s.src.join("Relato\u{301}rio.pdf")),
            p(&s.src.join("Fotos/../../Acme")),
            p(&root.join("nope")),
            "/".into(),
            p(&big),
            p(&root.join("link")),
        ])
        .await
        .unwrap();

    assert!(got[0].is_dir && got[0].problem.is_none());
    assert_eq!(got[0].children, Some(2));
    assert!(got[0].mtime.is_some());
    assert_eq!(got[0].covered_by, None);
    assert_eq!(got[1].covered_by, Some(0), "a folder inside another");
    assert_eq!(got[2].size, Some(6));
    assert_eq!(got[2].category, Some(FileCategory::Document));
    assert_eq!(got[2].covered_by, Some(0));
    assert_eq!(
        got[3].covered_by,
        Some(0),
        "the same folder written another way"
    );
    assert_eq!(got[4].problem, Some(SourceProblem::Missing));
    assert_eq!(got[5].problem, Some(SourceProblem::Root));
    assert_eq!(got[6].children, Some(1000));
    assert!(got[6].children_capped);
    #[cfg(unix)]
    {
        assert_eq!(got[7].problem, Some(SourceProblem::Symlink));
        assert_eq!(
            got[7].link_target.as_deref(),
            Some(p(&std::fs::canonicalize(s.src.join("Fotos")).unwrap()).as_str())
        );
    }
    assert_eq!(snapshot(&s.src), before, "the source is only looked at");
}

#[cfg(unix)]
#[tokio::test]
async fn an_unreadable_source_is_named() {
    use std::os::unix::fs::PermissionsExt;
    let s = setup().await;
    let locked = s.src.parent().unwrap().join("locked");
    std::fs::create_dir(&locked).unwrap();
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
    let readable_anyway = std::fs::read_dir(&locked).is_ok(); // running as root
    let got = s
        .engine
        .inspect_sources(vec![locked.display().to_string()])
        .await
        .unwrap();
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
    if !readable_anyway {
        assert_eq!(got[0].problem, Some(SourceProblem::Unreadable));
    }
}

#[tokio::test]
async fn nested_sources_are_refused() {
    let s = setup().await;
    for pair in [
        vec![s.src.clone(), s.src.join("Fotos")],
        vec![s.src.join("Fotos/Inverno.jpg"), s.src.clone()],
        vec![s.src.clone(), s.src.join("Fotos/../../Acme")],
    ] {
        let err = s
            .engine
            .create_job(pair, &s.profile_id, SendOptions::default())
            .await
            .unwrap_err();
        assert!(
            matches!(&err, CoreError::Invalid(m) if m.contains("already inside")),
            "{err}"
        );
    }
    // Two folders side by side are fine.
    s.engine
        .create_job(
            vec![s.src.join("Fotos"), s.src.join("Relato\u{301}rio.pdf")],
            &s.profile_id,
            SendOptions::default(),
        )
        .await
        .unwrap();
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

    // A profile change is app-level: it is in no job's log, but it is in Activity.
    assert!(kinds.iter().all(|k| *k != "profile"));
    let activity = s.engine.activity(None, 50).await.unwrap();
    let kinds: Vec<_> = activity.iter().map(|a| a.kind.as_str()).collect();
    assert_eq!(kinds, ["finished", "retry", "state", "created", "profile"]);
    assert_eq!(activity[0].job_label.as_deref(), Some("Acme"));
    assert_eq!(activity[0].profile_name.as_deref(), Some("Test"));
    assert_eq!(
        activity[4].profile_name.as_deref(),
        Some("Test"),
        "the profile change"
    );
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
async fn a_batch_sends_and_verifies_files_of_several_sources_in_one_call_each() {
    let s = setup().await;
    let other = s.src.parent().unwrap().join("Other");
    std::fs::create_dir_all(&other).unwrap();
    for i in 0..5 {
        std::fs::write(other.join(format!("o{i}.txt")), b"o").unwrap();
    }
    let single = s.src.parent().unwrap().join("single.txt");
    std::fs::write(&single, b"one").unwrap();
    let engine = engine_with_batch(
        s.store.clone(),
        s.transport.clone(),
        s.integration.clone(),
        100,
    );
    let sources = vec![s.src.join("Fotos"), other, single];
    let job = engine
        .create_job(
            sources,
            &s.profile_id,
            SendOptions {
                verify: true,
                ..SendOptions::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(engine.analyse(&job).await.unwrap().state, JobState::Ready);
    let (sends, hashes) = (
        s.transport.calls.load(Ordering::SeqCst),
        s.transport.hash_many_calls.load(Ordering::SeqCst),
    );
    engine.start(&job).await.unwrap();
    let state = tokio::time::timeout(Duration::from_secs(20), engine.wait(&job))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(state, JobState::Completed);
    // 8 files from 3 Sources: one transfer call and one hash_many, not one per Source.
    assert_eq!(s.transport.calls.load(Ordering::SeqCst) - sends, 1);
    assert_eq!(
        s.transport.hash_many_calls.load(Ordering::SeqCst) - hashes,
        1
    );
    let summary = engine.job(&job).await.unwrap();
    assert_eq!((summary.counts.done, summary.counts.verified_ok), (8, 8));
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
        target_dir: None,
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
async fn a_latin1_name_is_decoded_and_sent_readable() {
    use std::os::unix::ffi::OsStrExt;
    let s = setup().await;
    // "Patológica.pdf" written as Latin-1 (ó = f3); a name with a byte CP1252 leaves
    // undefined (81), which can only be guessed; and "Привет.txt" in CP1251, which CP1252 reads
    // as "Ïðèâåò.txt": not a Latin name, so a guess too.
    for raw in [
        &b"Patol\xf3gica.pdf"[..],
        b"guess\x81.txt",
        b"\xcf\xf0\xe8\xe2\xe5\xf2.txt",
    ] {
        std::fs::write(s.src.join(std::ffi::OsStr::from_bytes(raw)), b"x").unwrap();
    }
    let job = s
        .engine
        .create_job(vec![s.src.clone()], &s.profile_id, SendOptions::default())
        .await
        .unwrap();
    s.engine.analyse(&job).await.unwrap();
    // Keeping the original bytes is impossible: the destination takes UTF-8 only.
    assert!(matches!(
        s.engine
            .decide_all(&job, Some("encoding".into()), DecisionInput::KeepOriginal)
            .await,
        Err(CoreError::Invalid(_))
    ));
    // The sure reading is applied; the guesses wait for the user.
    assert_eq!(s.store.pending_issue_count(&job).await.unwrap(), 2);
    s.engine
        .decide_all(&job, Some("encoding".into()), DecisionInput::Accept)
        .await
        .unwrap();
    assert_eq!(run_to_end(&s, &job).await, JobState::Completed);
    let files = dest_files(&s.dest);
    assert!(
        files.contains(&"Acme/Patológica.pdf".to_owned()),
        "{files:?}"
    );
    assert!(
        files.contains(&"Acme/guess_.txt".to_owned()),
        "no control character reaches a name: {files:?}"
    );
}

#[tokio::test]
async fn a_garbled_folder_name_waits_for_a_decision() {
    let s = setup().await;
    // "Patológica" once read as Latin-1 and written back as UTF-8.
    let garbled = s.src.join("PatolÃ³gica");
    std::fs::create_dir(&garbled).unwrap();
    std::fs::write(garbled.join("a.txt"), b"x").unwrap();
    let job = s
        .engine
        .create_job(vec![s.src.clone()], &s.profile_id, SendOptions::default())
        .await
        .unwrap();
    let summary = s.engine.analyse(&job).await.unwrap();
    assert_eq!(summary.state, JobState::AwaitingApproval);
    let issues = s
        .engine
        .issues(&job, Some("mojibake".into()), None, 10)
        .await
        .unwrap();
    assert_eq!(issues.len(), 1);
    s.engine
        .decide_all(&job, Some("mojibake".into()), DecisionInput::Accept)
        .await
        .unwrap();
    assert_eq!(run_to_end(&s, &job).await, JobState::Completed);
    assert!(dest_files(&s.dest).contains(&"Acme/Patológica/a.txt".to_owned()));
}

#[tokio::test]
async fn names_already_at_the_destination_are_repaired_there() {
    let s = setup().await;
    // What an old rsync left at the destination: a garbled folder and an NFD file name.
    let fotos = s.dest.join("Fotos");
    std::fs::create_dir_all(fotos.join("PatolÃ³gica")).unwrap();
    std::fs::write(fotos.join("PatolÃ³gica/a.txt"), b"a").unwrap();
    std::fs::write(fotos.join("Vera\u{303}o.jpg"), b"v").unwrap();
    std::fs::write(fotos.join("ok.txt"), b"ok").unwrap();

    let job = s
        .engine
        .create_repair_job(&s.profile_id, Some("/Fotos/".into()))
        .await
        .unwrap();
    let summary = s.engine.analyse(&job).await.unwrap();
    assert_eq!(summary.state, JobState::AwaitingApproval, "{summary:?}");
    assert_eq!(summary.files, 3);
    let rules: Vec<_> = summary.rules.iter().map(|r| r.rule.as_str()).collect();
    assert!(
        rules.contains(&"mojibake") && rules.contains(&"nfc"),
        "{rules:?}"
    );
    assert!(matches!(
        s.engine.planned_renames(&job).await,
        Err(CoreError::PendingDecisions(1))
    ));
    s.engine
        .decide_all(&job, Some("mojibake".into()), DecisionInput::Accept)
        .await
        .unwrap();
    // The confirmation counts the entries that will change: the folder and the NFD file.
    assert_eq!(s.engine.planned_renames(&job).await.unwrap(), 2);
    // An entry's "source" is where its name was found at the destination.
    let detail = s.engine.entry_detail(&job, 0).await.unwrap();
    assert_eq!(
        detail.source_path,
        format!("{}/Fotos/PatolÃ³gica", s.dest.display())
    );
    // The folder and its decisions are what the user confirms: they can't be retargeted.
    assert!(matches!(
        s.engine
            .set_options(
                &job,
                SendOptions {
                    target_dir: Some("Elsewhere".into()),
                    ..SendOptions::default()
                }
            )
            .await,
        Err(CoreError::Invalid(_))
    ));

    let mut events = s.engine.subscribe();
    assert_eq!(run_to_end(&s, &job).await, JobState::Completed);
    let files = dest_files(&s.dest);
    assert_eq!(
        files,
        ["Fotos/Patológica/a.txt", "Fotos/Verão.jpg", "Fotos/ok.txt"],
        "renamed in place, nothing else touched"
    );
    // Progress counts files, like the job's counts.
    let mut last = None;
    while let Ok(e) = events.try_recv() {
        if let Event::TransferProgress {
            files_done,
            files_total,
            ..
        } = e
        {
            last = Some((files_done, files_total));
        }
    }
    assert_eq!(last, Some((3, 3)));
    assert_eq!(std::fs::read(fotos.join("Patológica/a.txt")).unwrap(), b"a");
    // Nextcloud-like tools rescan the folder that holds the renamed names.
    assert_eq!(*s.integration.changed.lock().unwrap(), ["Fotos"]);
    let job_view = s.engine.job(&job).await.unwrap();
    assert_eq!(job_view.kind, JobKind::Repair);
    assert_eq!(
        job_view.counts.done, 1,
        "files renamed (counts are of files)"
    );
    assert_eq!(s.store.pending_issue_count(&job).await.unwrap(), 0);
}

#[tokio::test]
async fn repair_never_overwrites_a_name_that_is_taken() {
    let s = setup().await;
    // A garbled name to repair; the repaired name only appears after the analysis, so the
    // name plan couldn't see the clash and the rename itself has to refuse it.
    std::fs::write(s.dest.join("PatolÃ³gica.txt"), b"garbled").unwrap();
    let job = s
        .engine
        .create_repair_job(&s.profile_id, None)
        .await
        .unwrap();
    s.engine.analyse(&job).await.unwrap();
    s.engine
        .decide_all(&job, Some("mojibake".into()), DecisionInput::Accept)
        .await
        .unwrap();
    // Simulate the clash appearing after the analysis.
    std::fs::write(s.dest.join("Patológica.txt"), b"arrived meanwhile").unwrap();
    assert_eq!(run_to_end(&s, &job).await, JobState::CompletedWithWarnings);
    assert_eq!(
        std::fs::read(s.dest.join("Patológica.txt")).unwrap(),
        b"arrived meanwhile"
    );
    assert_eq!(
        std::fs::read(s.dest.join("PatolÃ³gica.txt")).unwrap(),
        b"garbled"
    );
}

/// A Repair job for `dir`, analysed, with every `mojibake` and `collision` proposal accepted.
async fn repair_job(s: &Setup, dir: Option<&str>) -> String {
    let job = s
        .engine
        .create_repair_job(&s.profile_id, dir.map(Into::into))
        .await
        .unwrap();
    s.engine.analyse(&job).await.unwrap();
    for rule in ["mojibake", "collision"] {
        s.engine
            .decide_all(&job, Some(rule.into()), DecisionInput::Accept)
            .await
            .unwrap();
    }
    job
}

#[tokio::test]
async fn a_repair_runs_again_only_on_its_own_destination() {
    let s = setup().await;
    std::fs::create_dir_all(s.dest.join("Fotos")).unwrap();
    std::fs::write(s.dest.join("Fotos/PatolÃ³gica.txt"), b"garbled").unwrap();
    let job = repair_job(&s, Some("Fotos")).await;
    assert_eq!(run_to_end(&s, &job).await, JobState::Completed);
    let other = s
        .engine
        .save_profile(Profile {
            id: String::new(),
            name: "Other".into(),
            destination: Destination {
                transport: "fake".into(),
                backend: Backend::Local,
                root: s.src.to_string_lossy().into_owned(),
            },
            integration: None,
            integration_settings: serde_json::json!({}),
            target: TargetSpec::default(),
        })
        .await
        .unwrap();
    // The folder belongs to the first destination: another one is refused, not repaired.
    let err = s
        .engine
        .send_again(&job, Some(other.id.clone()))
        .await
        .unwrap_err();
    assert!(matches!(err, CoreError::Invalid(_)), "{err:?}");
    for profile in [None, Some(s.profile_id.clone())] {
        let again = s.engine.send_again(&job, profile).await.unwrap();
        let again = s.engine.job(&again).await.unwrap();
        assert_eq!(again.kind, JobKind::Repair);
        assert_eq!(again.profile_id, s.profile_id);
        assert_eq!(again.options.target_dir.as_deref(), Some("Fotos"));
    }
}

/// The warnings a finished job ended with.
async fn warnings(s: &Setup, job: &str) -> String {
    s.engine.job(job).await.unwrap().error.unwrap_or_default()
}

#[tokio::test]
async fn a_rename_cut_short_between_its_hops_is_finished_on_retry() {
    let s = setup().await;
    std::fs::write(s.dest.join("PatolÃ³gica.txt"), b"garbled").unwrap();
    let job = repair_job(&s, None).await;
    // The second hop fails as if the connection dropped: the file waits under its
    // intermediate name, and the retry finishes the move instead of losing it.
    s.transport.fail_move_once.lock().unwrap().insert(
        "Patológica.txt".into(),
        TransportError::retryable("connection reset by peer"),
    );
    assert_eq!(run_to_end(&s, &job).await, JobState::Completed);
    assert_eq!(dest_files(&s.dest), ["Patológica.txt"]);
    assert_eq!(s.engine.job(&job).await.unwrap().attempt, 1, "retried once");
}

#[tokio::test]
async fn a_rename_that_fails_halfway_goes_back_or_says_where_it_was_left() {
    let s = setup().await;
    std::fs::write(s.dest.join("PatolÃ³gica.txt"), b"garbled").unwrap();
    std::fs::write(s.dest.join("RelatÃ³rio.txt"), b"report").unwrap();
    let job = repair_job(&s, None).await;
    let fail = |name: &str| {
        s.transport
            .fail_move_once
            .lock()
            .unwrap()
            .insert(name.into(), TransportError::fatal("permission denied"));
    };
    // The first one's second hop fails for good: it is put back under its old name. The
    // second one can't be put back either: the warning says where it is.
    fail("Patológica.txt");
    fail("Relatório.txt");
    fail("RelatÃ³rio.txt");
    assert_eq!(run_to_end(&s, &job).await, JobState::CompletedWithWarnings);
    let files = dest_files(&s.dest);
    assert!(files.contains(&"PatolÃ³gica.txt".to_owned()), "{files:?}");
    let left: Vec<_> = files
        .iter()
        .filter(|f| f.starts_with(".fb-repair-"))
        .collect();
    assert_eq!(left.len(), 1, "{files:?}");
    let warned = warnings(&s, &job).await;
    assert!(
        warned.contains("RelatÃ³rio.txt is left as .fb-repair-"),
        "{warned}"
    );
    assert!(warned.contains("2 names could not be changed"), "{warned}");
}

#[tokio::test]
async fn repair_never_takes_a_name_in_another_case_or_normalisation() {
    let s = setup().await;
    std::fs::write(s.dest.join("PatolÃ³gica.txt"), b"garbled").unwrap();
    let job = repair_job(&s, None).await;
    // Appears after the analysis; a server that ignores case would store both as one.
    std::fs::write(s.dest.join("PATOLÓGICA.TXT"), b"other").unwrap();
    assert_eq!(run_to_end(&s, &job).await, JobState::CompletedWithWarnings);
    assert_eq!(
        dest_files(&s.dest),
        ["PATOLÓGICA.TXT", "PatolÃ³gica.txt"],
        "both kept as they were"
    );
}

#[tokio::test]
async fn a_folder_that_could_not_be_renamed_is_counted_and_rescanned_where_it_is() {
    let s = setup().await;
    let fotos = s.dest.join("Fotos");
    std::fs::create_dir_all(fotos.join("PatolÃ³gica")).unwrap();
    std::fs::write(fotos.join("PatolÃ³gica/Vera\u{303}o.jpg"), b"v").unwrap();
    let job = repair_job(&s, Some("Fotos")).await;
    // The folder's new name is taken after the analysis.
    std::fs::create_dir(fotos.join("Patológica")).unwrap();
    assert_eq!(run_to_end(&s, &job).await, JobState::CompletedWithWarnings);
    assert!(
        fotos.join("PatolÃ³gica/Verão.jpg").exists(),
        "{:?}",
        dest_files(&s.dest)
    );
    let warned = warnings(&s, &job).await;
    assert!(warned.contains("1 names could not be changed"), "{warned}");
    // The folder holding the renamed file is where it really is: under its old name.
    assert_eq!(
        *s.integration.changed.lock().unwrap(),
        ["Fotos/PatolÃ³gica"]
    );
}

#[tokio::test]
async fn a_name_renamed_by_hand_meanwhile_counts_only_if_it_is_the_same_file() {
    let s = setup().await;
    std::fs::write(s.dest.join("PatolÃ³gica.txt"), b"one").unwrap();
    std::fs::write(s.dest.join("RelatÃ³rio.txt"), b"two").unwrap();
    let job = repair_job(&s, None).await;
    // One is renamed by hand (the same file); the other replaced by a different file.
    std::fs::rename(
        s.dest.join("PatolÃ³gica.txt"),
        s.dest.join("Patológica.txt"),
    )
    .unwrap();
    std::fs::remove_file(s.dest.join("RelatÃ³rio.txt")).unwrap();
    std::fs::write(s.dest.join("Relatório.txt"), b"another file").unwrap();
    assert_eq!(run_to_end(&s, &job).await, JobState::CompletedWithWarnings);
    let warned = warnings(&s, &job).await;
    assert!(warned.contains("1 names could not be changed"), "{warned}");
    let job_view = s.engine.job(&job).await.unwrap();
    assert_eq!((job_view.counts.done, job_view.counts.failed), (1, 1));
}

#[tokio::test]
async fn renames_in_one_folder_free_a_name_before_it_is_taken() {
    let s = setup().await;
    // "Verão.txt" twice: NFD and NFC. The NFD one becomes "Verão.txt", which the NFC one has
    // to leave first (for "Verão (2).txt").
    std::fs::write(s.dest.join("Vera\u{303}o.txt"), b"nfd").unwrap();
    std::fs::write(s.dest.join("Ver\u{e3}o.txt"), b"nfc").unwrap();
    let job = repair_job(&s, None).await;
    assert_eq!(run_to_end(&s, &job).await, JobState::Completed);
    assert_eq!(dest_files(&s.dest), ["Verão (2).txt", "Verão.txt"]);
    assert_eq!(std::fs::read(s.dest.join("Verão.txt")).unwrap(), b"nfd");
    assert_eq!(std::fs::read(s.dest.join("Verão (2).txt")).unwrap(), b"nfc");
}

#[cfg(unix)]
#[tokio::test]
async fn an_unreadable_folder_is_a_scan_error_not_a_failed_analysis() {
    use std::os::unix::fs::PermissionsExt;
    let s = setup().await;
    let locked = s.dest.join("Fotos/locked");
    std::fs::create_dir_all(&locked).unwrap();
    std::fs::write(s.dest.join("Fotos/PatolÃ³gica.txt"), b"x").unwrap();
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
    let readable = std::fs::read_dir(&locked).is_ok(); // root reads it anyway
    let job = s
        .engine
        .create_repair_job(&s.profile_id, None)
        .await
        .unwrap();
    let summary = s.engine.analyse(&job).await;
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
    let summary = summary.unwrap();
    if !readable {
        assert_eq!(summary.scan_errors.len(), 1, "{:?}", summary.scan_errors);
        assert!(summary.scan_errors[0].contains("Fotos/locked"));
    }
    assert_eq!(
        summary.state,
        JobState::AwaitingApproval,
        "the rest is analysed"
    );
}

#[tokio::test]
async fn a_profile_password_lives_in_the_keyring_only() {
    use filebridge_secrets::{MemoryStore, SecretKind, SecretStore, reference};
    let store = Store::open_in_memory().unwrap();
    let secrets = Arc::new(MemoryStore::default());
    let mut registry = Registry::new();
    registry.add_transport(Arc::new(FakeTransport::default()));
    registry.set_secrets(secrets.clone());
    let engine = Engine::new(
        store.clone(),
        registry,
        EngineOptions {
            backoff: Backoff::Fixed(Duration::from_millis(10)),
            batch_size: 3,
            inhibit_sleep: false,
            max_verify_rounds: 3,
            local_clock: LocalClock::fixed(|| NOON),
        },
    );
    let profile = engine
        .save_profile(Profile {
            id: String::new(),
            name: "NAS".into(),
            destination: Destination {
                transport: "fake".into(),
                backend: Backend::Sftp {
                    host: "nas.example.lan".into(),
                    port: None,
                    user: Some("alice".into()),
                    key_file: None,
                    known_hosts_file: None,
                    password: None,
                    key: None,
                },
                root: "/srv/backup".into(),
            },
            integration: None,
            integration_settings: serde_json::json!({}),
            target: TargetSpec::default(),
        })
        .await
        .unwrap();
    let r = reference(&profile.id, SecretKind::AppPassword);

    let saved = engine
        .set_profile_password(&profile.id, Some("s3cret-pass".into()))
        .await
        .unwrap();
    assert!(
        matches!(&saved.destination.backend, Backend::Sftp { password: Some(p), .. } if *p == r)
    );
    assert_eq!(secrets.get(&r).unwrap().0.as_str(), "s3cret-pass");
    let rows = format!("{:?}", store.list_profiles().await.unwrap());
    assert!(
        !rows.contains("s3cret-pass"),
        "the database holds only the reference"
    );

    engine
        .set_profile_password(&profile.id, None)
        .await
        .unwrap();
    assert!(secrets.get(&r).is_err());
    let p = engine.get_profile(&profile.id).await.unwrap();
    assert!(matches!(
        p.destination.backend,
        Backend::Sftp { password: None, .. }
    ));

    engine
        .set_profile_password(&profile.id, Some("again".into()))
        .await
        .unwrap();
    engine.delete_profile(&profile.id).await.unwrap();
    assert!(
        secrets.get(&r).is_err(),
        "deleting the profile deletes its secrets"
    );
}

#[tokio::test]
async fn a_password_a_profile_needs_cannot_be_removed() {
    use filebridge_secrets::{MemoryStore, SecretKind, SecretStore, reference};
    let store = Store::open_in_memory().unwrap();
    let secrets = Arc::new(MemoryStore::default());
    let mut registry = Registry::new();
    registry.add_transport(Arc::new(FakeTransport::default()));
    registry.add_integration(Arc::new(FakeIntegration::default()));
    registry.set_secrets(secrets.clone());
    let engine = Engine::new(
        store.clone(),
        registry,
        EngineOptions {
            backoff: Backoff::Fixed(Duration::from_millis(10)),
            batch_size: 3,
            inhibit_sleep: false,
            max_verify_rounds: 3,
            local_clock: LocalClock::fixed(|| NOON),
        },
    );
    let id = "p-webdav".to_owned();
    let r = reference(&id, SecretKind::AppPassword);
    secrets.set(&r, "app-pass").unwrap();
    engine
        .save_profile(Profile {
            id: id.clone(),
            name: "Cloud".into(),
            destination: Destination {
                transport: "fake".into(),
                backend: Backend::Webdav {
                    url: "https://cloud.example.com/remote.php/dav/files/alice/".into(),
                    user: "alice".into(),
                    password: Some(r.clone()),
                    vendor: Some("nextcloud".into()),
                },
                root: "Backup".into(),
            },
            integration: Some("fakeint".into()),
            integration_settings: serde_json::json!({ "needs_password": true }),
            target: TargetSpec::default(),
        })
        .await
        .unwrap();
    let err = engine.set_profile_password(&id, None).await.unwrap_err();
    assert!(err.to_string().contains("needs its password"), "{err}");
    assert_eq!(
        secrets.get(&r).unwrap().0.as_str(),
        "app-pass",
        "refused before the keyring is touched"
    );
    // A new one is fine.
    engine
        .set_profile_password(&id, Some("new-app-pass".into()))
        .await
        .unwrap();
    assert_eq!(secrets.get(&r).unwrap().0.as_str(), "new-app-pass");
}

#[tokio::test]
async fn a_deleted_destination_keeps_its_jobs_history() {
    let s = setup().await;
    let draft = s
        .engine
        .create_job(vec![s.src.clone()], &s.profile_id, SendOptions::default())
        .await
        .unwrap();
    // Not while a job to it could still be sent or resumed.
    let err = s.engine.delete_profile(&s.profile_id).await.unwrap_err();
    assert!(err.to_string().contains("not finished (draft)"), "{err}");
    assert!(s.engine.get_profile(&s.profile_id).await.is_ok());
    s.engine.cancel(&draft).await.unwrap();

    let job = ready_job(&s, SendOptions::default()).await;
    assert_eq!(run_to_end(&s, &job).await, JobState::Completed);
    let profile = s.engine.get_profile(&s.profile_id).await.unwrap();
    s.engine.delete_profile(&s.profile_id).await.unwrap();

    // The report, a file's details and the activity still name it.
    let path = s.dest.parent().unwrap().join("after-delete.csv");
    let res = s
        .engine
        .export_report(
            &job,
            ReportRequest {
                format: ReportFormat::Csv,
                scope: ReportScope::All,
                include_dirs: false,
                path: path.display().to_string(),
            },
        )
        .await
        .unwrap();
    assert!(res.rows > 0);
    let page = s
        .engine
        .files_page(&job, EntryFilter::default(), None, 10)
        .await
        .unwrap();
    s.engine
        .entry_detail(&job, page.items[0].idx)
        .await
        .unwrap();
    let activity = s.engine.activity(None, 50).await.unwrap();
    assert!(
        activity
            .iter()
            .filter(|a| a.job_id.as_deref() == Some(job.as_str()))
            .all(|a| a.profile_name.as_deref() == Some("Test")),
        "{activity:?}"
    );

    // Nothing can be sent to it, and saving it again doesn't revive it unseen.
    assert!(
        s.engine
            .create_job(vec![s.src.clone()], &s.profile_id, SendOptions::default())
            .await
            .is_err()
    );
    let err = s.engine.save_profile(profile).await.unwrap_err();
    assert!(
        matches!(err, CoreError::Store(StoreError::Deleted(_))),
        "{err}"
    );
    assert!(s.engine.list_profiles().await.unwrap().is_empty());
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
async fn a_profile_is_busy_only_while_a_job_works_on_it() {
    let s = setup().await;
    s.engine.save_settings(closed_window()).await.unwrap();
    let job = ready_job(&s, SendOptions::default()).await;
    // Ready: not being worked on yet.
    assert!(
        s.engine
            .active_jobs_of(&s.profile_id)
            .await
            .unwrap()
            .is_empty()
    );
    s.engine.start(&job).await.unwrap();
    wait_for_state(&s.engine, &job, JobState::Waiting).await;
    assert_eq!(
        s.engine.active_jobs_of(&s.profile_id).await.unwrap(),
        vec![job.clone()]
    );
    assert!(s.engine.active_jobs_of("another").await.unwrap().is_empty());
    s.engine.cancel(&job).await.unwrap();
    assert!(
        s.engine
            .active_jobs_of(&s.profile_id)
            .await
            .unwrap()
            .is_empty()
    );
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
