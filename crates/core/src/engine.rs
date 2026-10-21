//! The engine's public API: profiles, jobs, analysis, decisions and the job lifecycle. The
//! pipeline itself (stages 4–6) is in `run.rs`.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use filebridge_integration_api::{JobContext, ProfileView};
use filebridge_store::{Decision, EntryState, JobRow, ManifestCounts, NewJob, Store};
use filebridge_transport_api::{CheckStatus, PreflightReport};
use serde::{Deserialize, Serialize};
use tokio::sync::{Mutex, broadcast};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::analyse::{self, bytes_path, os_bytes};
use crate::backoff::Backoff;
use crate::paths::{self, Node, Planned};
use crate::{CoreError, Event, JobKind, JobState, Profile, Registry, Result};

const INSERT_BATCH: usize = 10_000;
const PAGE: u32 = 5_000;

#[derive(Debug, Clone)]
pub struct EngineOptions {
    pub backoff: Backoff,
    /// Files per transport call.
    pub batch_size: u32,
    pub inhibit_sleep: bool,
    /// Verify → resend rounds before giving up on mismatches.
    pub max_verify_rounds: u32,
}

impl Default for EngineOptions {
    fn default() -> Self {
        Self {
            backoff: Backoff::Schedule,
            batch_size: 500,
            inhibit_sleep: true,
            max_verify_rounds: 3,
        }
    }
}

/// Per-job choices made in step 4 of the wizard.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type)]
pub struct SendOptions {
    /// Compare SHA-256 of every file at the end (source vs destination).
    pub verify: bool,
    /// Files in flight at once.
    pub concurrency: u32,
    /// rclone-style bandwidth limit, e.g. `"20M"`.
    pub bwlimit: Option<String>,
}

impl Default for SendOptions {
    fn default() -> Self {
        Self {
            verify: false,
            concurrency: 8,
            bwlimit: None,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub struct Counts {
    pub files: u64,
    pub dirs: u64,
    pub bytes: u64,
    pub pending: u64,
    pub done: u64,
    pub failed: u64,
    pub skipped: u64,
    pub bytes_done: u64,
    pub verified_ok: u64,
    pub mismatch: u64,
}

impl From<ManifestCounts> for Counts {
    fn from(c: ManifestCounts) -> Self {
        Self {
            files: c.files,
            dirs: c.dirs,
            bytes: c.bytes,
            pending: c.pending + c.partial,
            done: c.done,
            failed: c.failed,
            skipped: c.skipped,
            bytes_done: c.bytes_done,
            verified_ok: c.verified_ok,
            mismatch: c.mismatch + c.missing,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type)]
pub struct JobSummary {
    pub id: String,
    pub kind: JobKind,
    pub profile_id: String,
    pub state: JobState,
    pub error: Option<String>,
    pub attempt: u32,
    pub next_retry_at: Option<i64>,
    pub created_at: i64,
    pub updated_at: i64,
    pub finished_at: Option<i64>,
    /// Source roots, lossily decoded for display.
    pub sources: Vec<String>,
    pub options: SendOptions,
    pub counts: Counts,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type)]
pub struct CheckView {
    pub id: String,
    /// `ok`, `warn` or `fail`.
    pub status: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type)]
pub struct PreflightView {
    /// `transport` or `integration`.
    pub source: String,
    pub free_bytes: Option<u64>,
    pub writable: bool,
    pub checks: Vec<CheckView>,
}

impl PreflightView {
    fn from_report(source: &str, r: PreflightReport) -> Self {
        Self {
            source: source.into(),
            free_bytes: r.free_bytes,
            writable: r.writable,
            checks: r
                .checks
                .into_iter()
                .map(|c| CheckView {
                    id: c.id,
                    status: match c.status {
                        CheckStatus::Ok => "ok",
                        CheckStatus::Warn => "warn",
                        CheckStatus::Fail => "fail",
                    }
                    .into(),
                    message: c.message,
                })
                .collect(),
        }
    }

    fn failed(source: &str, message: String) -> Self {
        Self {
            source: source.into(),
            free_bytes: None,
            writable: false,
            checks: vec![CheckView {
                id: format!("{source}.preflight"),
                status: "fail".into(),
                message,
            }],
        }
    }

    pub fn has_failures(&self) -> bool {
        self.checks.iter().any(|c| c.status == "fail")
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub struct RuleCount {
    pub rule: String,
    pub total: u64,
    pub pending: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type)]
pub struct AnalysisSummary {
    pub job_id: String,
    pub state: JobState,
    pub files: u64,
    pub dirs: u64,
    pub bytes: u64,
    pub rules: Vec<RuleCount>,
    pub pending_decisions: u64,
    pub scan_errors: Vec<String>,
    pub skipped_symlinks: u64,
    pub preflight: Vec<PreflightView>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type)]
pub struct IssueView {
    pub id: i64,
    pub entry_idx: u64,
    /// Source path relative to the source's parent, lossily decoded.
    pub path: String,
    pub rule: String,
    /// `auto` or `needs_decision`.
    pub severity: String,
    pub message_key: String,
    pub before: Vec<u8>,
    pub after: Vec<u8>,
    pub before_text: String,
    pub after_text: String,
    /// `pending`, `accepted`, `edited`, `rejected` or `skipped`.
    pub decision: String,
    pub custom_name: Option<String>,
}

/// What the user decides about one issue.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(tag = "kind", content = "name", rename_all = "snake_case")]
pub enum DecisionInput {
    Accept,
    Edit(String),
    KeepOriginal,
    Skip,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type)]
pub struct ProfileTest {
    pub transport: PreflightView,
    pub integration: Option<PreflightView>,
}

struct Running {
    generation: u64,
    cancel: CancellationToken,
    handle: JoinHandle<()>,
}

pub struct Engine {
    pub(crate) store: Store,
    pub(crate) registry: Arc<Registry>,
    pub(crate) events: broadcast::Sender<Event>,
    pub(crate) options: EngineOptions,
    running: Mutex<HashMap<String, Running>>,
    generation: AtomicU64,
}

impl Engine {
    pub fn new(store: Store, registry: Registry, options: EngineOptions) -> Arc<Self> {
        let (events, _) = broadcast::channel(1024);
        Arc::new(Self {
            store,
            registry: Arc::new(registry),
            events,
            options,
            running: Mutex::new(HashMap::new()),
            generation: AtomicU64::new(0),
        })
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.events.subscribe()
    }

    pub fn registry(&self) -> &Registry {
        &self.registry
    }

    pub(crate) fn emit(&self, event: Event) {
        let _ = self.events.send(event);
    }

    // ---- profiles ----------------------------------------------------------------------

    /// Saves a profile, giving it an id if it has none. Rejects unknown plugins and
    /// transport/integration combinations that can't work.
    pub async fn save_profile(&self, mut profile: Profile) -> Result<Profile> {
        if profile.id.is_empty() {
            profile.id = uuid::Uuid::new_v4().to_string();
        }
        self.registry.check_profile(&profile)?;
        self.store.upsert_profile(profile.to_row()?).await?;
        Ok(profile)
    }

    pub async fn get_profile(&self, id: &str) -> Result<Profile> {
        Profile::from_row(self.store.get_profile(id).await?)
    }

    pub async fn list_profiles(&self) -> Result<Vec<Profile>> {
        self.store
            .list_profiles()
            .await?
            .into_iter()
            .map(Profile::from_row)
            .collect()
    }

    pub async fn delete_profile(&self, id: &str) -> Result<()> {
        Ok(self.store.delete_profile(id).await?)
    }

    /// Connection test: the transport's and the integration's preflight.
    pub async fn test_profile(&self, id: &str) -> Result<ProfileTest> {
        let profile = self.get_profile(id).await?;
        let preflight = self.preflight(&profile, "profile-test").await?;
        let mut it = preflight.into_iter();
        Ok(ProfileTest {
            transport: it.next().expect("transport preflight is always present"),
            integration: it.next(),
        })
    }

    pub(crate) fn job_context(&self, job_id: &str, profile: &Profile) -> Result<JobContext> {
        let transport = self.registry.transport(&profile.destination.transport)?;
        Ok(JobContext {
            job_id: job_id.to_owned(),
            profile: ProfileView {
                id: profile.id.clone(),
                name: profile.name.clone(),
                destination: profile.destination.clone(),
                settings: profile.integration_settings.clone(),
            },
            caps: transport.capabilities(),
        })
    }

    async fn preflight(&self, profile: &Profile, job_id: &str) -> Result<Vec<PreflightView>> {
        let transport = self.registry.transport(&profile.destination.transport)?;
        let mut out = vec![match transport.preflight(&profile.destination).await {
            Ok(r) => PreflightView::from_report("transport", r),
            Err(e) => PreflightView::failed("transport", e.message),
        }];
        if let Some(id) = &profile.integration {
            let integration = self.registry.integration(id)?;
            let ctx = self.job_context(job_id, profile)?;
            out.push(match integration.preflight(&ctx).await {
                Ok(r) => PreflightView::from_report("integration", r),
                Err(e) => PreflightView::failed("integration", e.message),
            });
        }
        Ok(out)
    }

    // ---- jobs --------------------------------------------------------------------------

    /// Creates a draft job. Nothing is read from the sources yet.
    pub async fn create_job(
        &self,
        sources: Vec<PathBuf>,
        profile_id: &str,
        options: SendOptions,
    ) -> Result<String> {
        if sources.is_empty() {
            return Err(CoreError::Invalid(
                "pick at least one file or folder".into(),
            ));
        }
        let profile = self.get_profile(profile_id).await?;
        self.registry.check_profile(&profile)?;
        let mut roots = Vec::with_capacity(sources.len());
        for s in &sources {
            let abs = std::path::absolute(s)?;
            if abs.file_name().is_none() {
                return Err(CoreError::Invalid(format!(
                    "{} cannot be sent",
                    abs.display()
                )));
            }
            roots.push(os_bytes(abs.as_os_str()));
        }
        let id = uuid::Uuid::new_v4().to_string();
        self.store
            .insert_job(NewJob {
                id: id.clone(),
                kind: JobKind::Transfer.as_str().into(),
                profile_id: profile.id,
                state: JobState::Draft.as_str().into(),
                options: serde_json::to_value(&options)?,
                sources: roots,
            })
            .await?;
        Ok(id)
    }

    pub async fn job(&self, id: &str) -> Result<JobSummary> {
        let row = self.store.get_job(id).await?;
        self.summary(row).await
    }

    /// Newest first.
    pub async fn list_jobs(&self, limit: u32, offset: u32) -> Result<Vec<JobSummary>> {
        let mut out = Vec::new();
        for row in self.store.list_jobs(limit, offset).await? {
            out.push(self.summary(row).await?);
        }
        Ok(out)
    }

    async fn summary(&self, row: JobRow) -> Result<JobSummary> {
        let counts = self.store.manifest_counts(&row.id).await?.into();
        Ok(JobSummary {
            kind: JobKind::parse(&row.kind)?,
            state: JobState::parse(&row.state)?,
            options: serde_json::from_value(row.options).unwrap_or_default(),
            sources: row
                .sources
                .iter()
                .map(|s| String::from_utf8_lossy(s).into_owned())
                .collect(),
            id: row.id,
            profile_id: row.profile_id,
            error: row.error,
            attempt: row.attempt,
            next_retry_at: row.next_retry_at,
            created_at: row.created_at,
            updated_at: row.updated_at,
            finished_at: row.finished_at,
            counts,
        })
    }

    pub(crate) async fn job_sources(&self, row: &JobRow) -> Vec<PathBuf> {
        row.sources.iter().map(|s| bytes_path(s)).collect()
    }

    /// Records a state change (validated), persists it and tells subscribers.
    pub(crate) async fn transition(
        &self,
        job_id: &str,
        to: JobState,
        error: Option<String>,
    ) -> Result<JobState> {
        let from = JobState::parse(&self.store.get_job(job_id).await?.state)?;
        if from == to {
            return Ok(from);
        }
        if !from.can_go_to(to) {
            return Err(CoreError::InvalidTransition { from, to });
        }
        if to.is_terminal() {
            self.store
                .finish_job(job_id, to.as_str(), error.clone())
                .await?;
        } else {
            self.store
                .set_job_state(job_id, to.as_str(), error.clone())
                .await?;
        }
        self.store
            .append_event(
                Some(job_id),
                "state",
                serde_json::json!({ "from": from, "to": to, "error": error }),
            )
            .await?;
        self.emit(Event::JobStateChanged {
            job_id: job_id.to_owned(),
            from,
            to,
            error: error.clone(),
        });
        if to.is_terminal() {
            self.emit(Event::JobFinished {
                job_id: job_id.to_owned(),
                state: to,
                error,
            });
        }
        Ok(from)
    }

    // ---- stages 2–3: analysis and decisions ---------------------------------------------

    /// Scans the sources, runs the name rules and the preflight checks. Re-running replaces
    /// the previous analysis and its decisions.
    pub async fn analyse(&self, job_id: &str) -> Result<AnalysisSummary> {
        let row = self.store.get_job(job_id).await?;
        let state = JobState::parse(&row.state)?;
        if !matches!(
            state,
            JobState::Draft | JobState::AwaitingApproval | JobState::Ready
        ) {
            return Err(CoreError::InvalidTransition {
                from: state,
                to: JobState::Analysing,
            });
        }
        self.transition(job_id, JobState::Analysing, None).await?;
        match self.analyse_inner(&row).await {
            Ok(summary) => Ok(summary),
            Err(e) => {
                self.transition(job_id, JobState::Draft, Some(e.to_string()))
                    .await?;
                Err(e)
            }
        }
    }

    async fn analyse_inner(&self, row: &JobRow) -> Result<AnalysisSummary> {
        let job_id = row.id.clone();
        let profile = self.get_profile(&row.profile_id).await?;
        let rules = self.registry.rule_set(&profile)?;
        let sources = self.job_sources(row).await;
        let case_sensitive = profile.target.case_sensitive;

        self.store.clear_issues(&job_id).await?;
        self.store.clear_manifest(&job_id).await?;

        let events = self.events.clone();
        let jid = job_id.clone();
        let analysis = tokio::task::spawn_blocking(move || {
            let (mut files, mut bytes) = (0, 0);
            analyse::analyse_sources(&sources, &rules, case_sensitive, |src, f, b| {
                files += f;
                bytes += b;
                let _ = events.send(Event::ScanProgress {
                    job_id: jid.clone(),
                    files,
                    bytes,
                    current: Some(src.display().to_string()),
                });
            })
        })
        .await
        .map_err(|e| CoreError::Invalid(format!("analysis task failed: {e}")))??;

        let mut entries = analysis.entries;
        while !entries.is_empty() {
            let rest = entries.split_off(entries.len().min(INSERT_BATCH));
            self.store.insert_entries(&job_id, entries).await?;
            entries = rest;
        }
        self.store
            .set_dest_paths(&job_id, analysis.dest_paths)
            .await?;
        self.store.insert_issues(&job_id, analysis.issues).await?;

        let preflight = self.preflight(&profile, &job_id).await?;
        let pending = self.store.pending_issue_count(&job_id).await?;
        let next = if pending > 0 {
            JobState::AwaitingApproval
        } else {
            JobState::Ready
        };
        self.store
            .append_event(
                Some(&job_id),
                "analysis",
                serde_json::json!({
                    "files": analysis.files, "bytes": analysis.bytes,
                    "scan_errors": analysis.scan_errors.len(),
                    "skipped_symlinks": analysis.skipped_symlinks,
                    "preflight": preflight,
                }),
            )
            .await?;
        self.transition(&job_id, next, None).await?;
        Ok(AnalysisSummary {
            job_id: job_id.clone(),
            state: next,
            files: analysis.files,
            dirs: analysis.dirs,
            bytes: analysis.bytes,
            rules: self.rule_counts(&job_id).await?,
            pending_decisions: pending,
            scan_errors: analysis.scan_errors,
            skipped_symlinks: analysis.skipped_symlinks,
            preflight,
        })
    }

    pub async fn rule_counts(&self, job_id: &str) -> Result<Vec<RuleCount>> {
        Ok(self
            .store
            .issue_summary(job_id)
            .await?
            .into_iter()
            .map(|s| RuleCount {
                rule: s.rule,
                total: s.total,
                pending: s.pending,
            })
            .collect())
    }

    /// Issues of a job (optionally one rule), by id after `after_id`.
    pub async fn issues(
        &self,
        job_id: &str,
        rule: Option<String>,
        after_id: Option<i64>,
        limit: u32,
    ) -> Result<Vec<IssueView>> {
        let rows = self
            .store
            .issues_page(job_id, rule, after_id, limit)
            .await?;
        let mut out = Vec::with_capacity(rows.len());
        for r in rows {
            let entry = self
                .store
                .entries_page(job_id, r.issue.entry_idx.checked_sub(1), 1)
                .await?;
            let path = entry
                .first()
                .map(|e| String::from_utf8_lossy(&e.entry.raw_path).into_owned())
                .unwrap_or_default();
            out.push(IssueView {
                id: r.id,
                entry_idx: r.issue.entry_idx,
                path,
                before_text: String::from_utf8_lossy(&r.issue.before).into_owned(),
                after_text: String::from_utf8_lossy(&r.issue.after).into_owned(),
                rule: r.issue.rule,
                severity: r.issue.severity,
                message_key: r.issue.message_key,
                before: r.issue.before,
                after: r.issue.after,
                decision: r.issue.decision.as_str().into(),
                custom_name: r.custom_name,
            });
        }
        Ok(out)
    }

    /// Records a decision; the job becomes Ready once nothing is pending.
    pub async fn decide_issue(
        &self,
        job_id: &str,
        issue_id: i64,
        input: DecisionInput,
    ) -> Result<()> {
        let state = JobState::parse(&self.store.get_job(job_id).await?.state)?;
        if !matches!(state, JobState::AwaitingApproval | JobState::Ready) {
            return Err(CoreError::Invalid(format!(
                "decisions can't change while the job is {}",
                state.as_str()
            )));
        }
        let (decision, name) = match input {
            DecisionInput::Accept => (Decision::Accepted, None),
            DecisionInput::Edit(n) => (Decision::Edited, Some(n)),
            DecisionInput::KeepOriginal => (Decision::Rejected, None),
            DecisionInput::Skip => (Decision::Skipped, None),
        };
        self.store.decide_issue(issue_id, decision, name).await?;
        if state == JobState::AwaitingApproval && self.store.pending_issue_count(job_id).await? == 0
        {
            self.transition(job_id, JobState::Ready, None).await?;
        }
        Ok(())
    }

    /// Applies the decisions to the manifest: final destination paths and skipped entries.
    async fn plan_destinations(&self, job_id: &str, case_sensitive: bool) -> Result<()> {
        let mut nodes = Vec::new();
        let mut after = None;
        loop {
            let page = self.store.entries_page(job_id, after, PAGE).await?;
            let Some(last) = page.last() else { break };
            after = Some(last.entry.idx);
            for e in page {
                if e.entry.idx != nodes.len() as u64 {
                    return Err(CoreError::Invalid("manifest has gaps".into()));
                }
                nodes.push(Node {
                    parent: e.entry.parent_idx,
                    raw_path: e.entry.raw_path,
                    dest_path: e.dest_path,
                });
            }
        }
        let mut issues = Vec::new();
        let mut after = None;
        loop {
            let page = self.store.issues_page(job_id, None, after, PAGE).await?;
            let Some(last) = page.last() else { break };
            after = Some(last.id);
            issues.extend(page);
        }
        let overrides = paths::overrides(&nodes, &issues)?;
        let planned = paths::plan(&nodes, &overrides, case_sensitive)?;
        let mut dest = Vec::new();
        let mut skipped = Vec::new();
        for (i, p) in planned.into_iter().enumerate() {
            match p {
                Planned::Path(path) => dest.push((i as u64, Some(path))),
                Planned::Skipped => skipped.push((i as u64, EntryState::Skipped, None)),
            }
        }
        self.store.set_dest_paths(job_id, dest).await?;
        self.store.set_transfer_states(job_id, skipped).await?;
        Ok(())
    }

    // ---- lifecycle -----------------------------------------------------------------------

    /// Sends a Ready job (or resumes a Paused one) in the background.
    pub async fn start(self: &Arc<Self>, job_id: &str) -> Result<()> {
        let row = self.store.get_job(job_id).await?;
        match JobState::parse(&row.state)? {
            JobState::Ready => {
                let profile = self.get_profile(&row.profile_id).await?;
                self.plan_destinations(job_id, profile.target.case_sensitive)
                    .await?;
            }
            JobState::Paused => {
                self.store
                    .reset_transfer_states(job_id, &[EntryState::Partial], EntryState::Pending)
                    .await?;
            }
            JobState::AwaitingApproval => {
                let n = self.store.pending_issue_count(job_id).await?;
                return Err(CoreError::PendingDecisions(n));
            }
            other => {
                return Err(CoreError::InvalidTransition {
                    from: other,
                    to: JobState::Transferring,
                });
            }
        }
        self.transition(job_id, JobState::Transferring, None)
            .await?;
        self.spawn_run(job_id).await;
        Ok(())
    }

    /// Same as [`Engine::start`] on a Paused job.
    pub async fn resume(self: &Arc<Self>, job_id: &str) -> Result<()> {
        self.start(job_id).await
    }

    /// Stops the runner; the job keeps its progress and can be resumed.
    pub async fn pause(&self, job_id: &str) -> Result<()> {
        self.stop_runner(job_id).await;
        let state = JobState::parse(&self.store.get_job(job_id).await?.state)?;
        if !state.is_running() {
            return Err(CoreError::InvalidTransition {
                from: state,
                to: JobState::Paused,
            });
        }
        self.store
            .reset_transfer_states(job_id, &[EntryState::Partial], EntryState::Pending)
            .await?;
        self.transition(job_id, JobState::Paused, None).await?;
        Ok(())
    }

    /// Stops the job for good. Nothing at the source or destination is deleted.
    pub async fn cancel(&self, job_id: &str) -> Result<()> {
        self.stop_runner(job_id).await;
        self.transition(job_id, JobState::Cancelled, None).await?;
        Ok(())
    }

    /// On startup: restarts every job that was running when the app stopped. Returns their ids.
    pub async fn resume_unfinished(self: &Arc<Self>) -> Result<Vec<String>> {
        let mut resumed = Vec::new();
        for row in self.store.list_unfinished_jobs().await? {
            match JobState::parse(&row.state)? {
                JobState::Analysing => {
                    self.transition(
                        &row.id,
                        JobState::Draft,
                        Some("the analysis was interrupted; run it again".into()),
                    )
                    .await?;
                }
                st if st.is_running() => {
                    self.store
                        .reset_transfer_states(&row.id, &[EntryState::Partial], EntryState::Pending)
                        .await?;
                    self.spawn_run(&row.id).await;
                    resumed.push(row.id);
                }
                _ => {}
            }
        }
        Ok(resumed)
    }

    /// Waits until the job is no longer being worked on (finished, paused, or awaiting the
    /// user) and returns its state.
    pub async fn wait(&self, job_id: &str) -> Result<JobState> {
        let mut rx = self.subscribe();
        loop {
            let state = JobState::parse(&self.store.get_job(job_id).await?.state)?;
            let active = self.running.lock().await.contains_key(job_id);
            if state.is_terminal() || (!active && !state.is_running()) {
                return Ok(state);
            }
            let _ = tokio::time::timeout(Duration::from_millis(500), rx.recv()).await;
        }
    }

    pub(crate) async fn spawn_run(self: &Arc<Self>, job_id: &str) {
        let mut running = self.running.lock().await;
        if running.contains_key(job_id) {
            return;
        }
        let generation = self.generation.fetch_add(1, Ordering::Relaxed);
        let cancel = CancellationToken::new();
        let engine = Arc::clone(self);
        let (id, token) = (job_id.to_owned(), cancel.clone());
        let handle = tokio::spawn(async move {
            engine.run(&id, &token).await;
            let mut running = engine.running.lock().await;
            if running.get(&id).is_some_and(|r| r.generation == generation) {
                running.remove(&id);
            }
        });
        running.insert(
            job_id.to_owned(),
            Running {
                generation,
                cancel,
                handle,
            },
        );
    }

    async fn stop_runner(&self, job_id: &str) {
        let taken = self.running.lock().await.remove(job_id);
        if let Some(r) = taken {
            r.cancel.cancel();
            let _ = r.handle.await;
        }
    }
}
