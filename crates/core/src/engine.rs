//! The engine's public API: profiles, jobs, analysis, decisions and the job lifecycle. The
//! pipeline itself (stages 4–6) is in `run.rs`.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use filebridge_integration_api::{JobContext, ProfileView};
use filebridge_model::AppSettings;
use filebridge_names::{Choice, Entry, NamePlan, Planned};
use filebridge_store::{Decision, EntryState, IssueRow, JobRow, ManifestCounts, NewJob, Store};
use tokio::sync::{Mutex, broadcast, watch};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::analyse::{self, bytes_path, os_bytes};
use crate::backoff::Backoff;
use crate::decisions;
use crate::lifecycle::{JobLock, JobLocks};
use crate::settings::LocalClock;
use crate::{
    AnalysisSummary, CoreError, Counts, DecisionInput, Event, IssueView, JobKind, JobState,
    JobSummary, PreflightView, Profile, ProfileTest, Registry, Result, RuleCount, SendOptions,
};
use filebridge_model::{
    ActivityItem, CategoryCount, DecideManyResult, FileCategory, LogEntry, Op, RecentSource,
    RefusedIssue, default_label, log_level,
};

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
    /// The local time the send window is checked against.
    pub local_clock: LocalClock,
}

impl Default for EngineOptions {
    fn default() -> Self {
        Self {
            backoff: Backoff::Schedule,
            batch_size: 500,
            inhibit_sleep: true,
            max_verify_rounds: 3,
            local_clock: LocalClock::default(),
        }
    }
}

/// A manifest's totals as the UI shows them: partial files count as pending, missing ones as
/// mismatched.
fn counts(c: ManifestCounts) -> Counts {
    Counts {
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
    pub(crate) settings: watch::Sender<AppSettings>,
    running: Mutex<HashMap<String, Running>>,
    generation: AtomicU64,
    pub(crate) locks: JobLocks,
}

impl Engine {
    pub fn new(store: Store, registry: Registry, options: EngineOptions) -> Arc<Self> {
        let (events, _) = broadcast::channel(1024);
        Arc::new(Self {
            store,
            registry: Arc::new(registry),
            events,
            options,
            settings: watch::Sender::new(AppSettings::default()),
            running: Mutex::new(HashMap::new()),
            generation: AtomicU64::new(0),
            locks: JobLocks::default(),
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

    /// Appends a milestone to the job log (`job_id = None`: app-level, e.g. a profile change).
    /// `data` holds parameters only, never sentences or secrets (docs/design/specs/D07).
    pub(crate) async fn log(
        &self,
        job_id: Option<&str>,
        kind: &str,
        data: serde_json::Value,
    ) -> Result<()> {
        self.store.append_event(job_id, kind, data).await?;
        Ok(())
    }

    /// Milestones of every job and profile changes, newest first, before `before_id`.
    pub async fn activity(&self, before_id: Option<i64>, limit: u32) -> Result<Vec<ActivityItem>> {
        let rows = self.store.activity_page(before_id, limit.min(200)).await?;
        let mut jobs: HashMap<String, Option<(String, String)>> = HashMap::new();
        let mut profile_names: HashMap<String, Option<String>> = HashMap::new();
        let mut out = Vec::with_capacity(rows.len());
        for r in rows {
            let (job_label, profile_name) = match &r.job_id {
                Some(id) => {
                    if !jobs.contains_key(id) {
                        let found = match self.store.get_job(id).await {
                            Ok(j) => {
                                let sources: Vec<String> = j
                                    .sources
                                    .iter()
                                    .map(|s| String::from_utf8_lossy(s).into_owned())
                                    .collect();
                                Some((
                                    j.label.unwrap_or_else(|| default_label(&sources)),
                                    j.profile_id,
                                ))
                            }
                            Err(_) => None,
                        };
                        jobs.insert(id.clone(), found);
                    }
                    match jobs[id].clone() {
                        Some((label, profile_id)) => {
                            if !profile_names.contains_key(&profile_id) {
                                let name = self.get_profile(&profile_id).await.ok().map(|p| p.name);
                                profile_names.insert(profile_id.clone(), name);
                            }
                            (Some(label), profile_names[&profile_id].clone())
                        }
                        None => (None, None),
                    }
                }
                None => (
                    None,
                    r.data
                        .get("name")
                        .and_then(|v| v.as_str())
                        .map(str::to_owned),
                ),
            };
            out.push(ActivityItem {
                level: log_level(&r.kind, &r.data),
                id: r.id,
                at: r.at,
                kind: r.kind,
                job_id: r.job_id,
                job_label,
                profile_name,
                data: r.data,
            });
        }
        Ok(out)
    }

    /// A job's files per Category, most bytes first (what the transfer contains).
    pub async fn job_categories(&self, job_id: &str) -> Result<Vec<CategoryCount>> {
        self.store.get_job(job_id).await?;
        Ok(self
            .store
            .category_counts(job_id)
            .await?
            .into_iter()
            .map(|(c, files, bytes)| CategoryCount {
                category: FileCategory::parse(&c).unwrap_or(FileCategory::Other),
                files,
                bytes,
            })
            .collect())
    }

    /// Folders and files sent before, newest first, one per path, each checked for whether it
    /// is still there. Paths that aren't valid text are left out (a job is created from text).
    pub async fn recent_sources(&self, limit: u32) -> Result<Vec<RecentSource>> {
        let rows = self.store.recent_sources(limit).await?;
        Ok(rows
            .into_iter()
            .filter_map(|r| {
                let path = String::from_utf8(r.path).ok()?;
                let exists = std::path::Path::new(&path).exists();
                Some(RecentSource {
                    path,
                    last_job_id: r.job_id,
                    last_profile_id: r.profile_id,
                    last_sent_at: r.sent_at,
                    exists,
                })
            })
            .collect())
    }

    /// A job's log: oldest first after `after_id`, otherwise newest first before `before_id`
    /// (the newest page when both are `None`).
    pub async fn job_log(
        &self,
        job_id: &str,
        after_id: Option<i64>,
        before_id: Option<i64>,
        limit: u32,
    ) -> Result<Vec<LogEntry>> {
        self.store.get_job(job_id).await?;
        let rows = match after_id {
            Some(after) => self.store.events_page(job_id, Some(after), limit).await?,
            None => {
                self.store
                    .events_page_before(job_id, before_id, limit)
                    .await?
            }
        };
        Ok(rows
            .into_iter()
            .map(|r| LogEntry {
                level: log_level(&r.kind, &r.data),
                id: r.id,
                at: r.at,
                kind: r.kind,
                data: r.data,
            })
            .collect())
    }

    // ---- profiles ----------------------------------------------------------------------

    /// Saves a profile, giving it an id if it has none. Rejects unknown plugins and
    /// transport/integration combinations that can't work.
    pub async fn save_profile(&self, mut profile: Profile) -> Result<Profile> {
        let new = profile.id.is_empty();
        if new {
            profile.id = uuid::Uuid::new_v4().to_string();
        }
        self.registry.check_profile(&profile)?;
        self.store
            .upsert_profile(crate::profile::to_row(&profile)?)
            .await?;
        if new {
            self.log(None, "profile", profile_event("added", &profile))
                .await?;
        }
        Ok(profile)
    }

    pub async fn get_profile(&self, id: &str) -> Result<Profile> {
        crate::profile::from_row(self.store.get_profile(id).await?)
    }

    pub async fn list_profiles(&self) -> Result<Vec<Profile>> {
        self.store
            .list_profiles()
            .await?
            .into_iter()
            .map(crate::profile::from_row)
            .collect()
    }

    pub async fn delete_profile(&self, id: &str) -> Result<()> {
        let profile = self.get_profile(id).await.ok();
        self.store.delete_profile(id).await?;
        if let Some(p) = profile {
            self.log(None, "profile", profile_event("removed", &p))
                .await?;
        }
        Ok(())
    }

    /// Connection test: the transport's and the integration's preflight.
    pub async fn test_profile(&self, id: &str) -> Result<ProfileTest> {
        let profile = self.get_profile(id).await?;
        self.measure(&profile, "profile-test").await
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

    pub(crate) async fn preflight(
        &self,
        profile: &Profile,
        job_id: &str,
    ) -> Result<Vec<PreflightView>> {
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
        self.check_target_dir(&profile, options.target_dir.as_deref())?;
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
        // A folder inside another chosen one would be sent twice.
        if let Some((inside, container)) = crate::sources::first_nested(&sources) {
            return Err(CoreError::Invalid(format!(
                "{} is already inside {}",
                inside.display(),
                container.display()
            )));
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
                label: None,
                again_of: None,
            })
            .await?;
        let sources: Vec<String> = sources.iter().map(|s| s.display().to_string()).collect();
        self.log(
            Some(&id),
            "created",
            serde_json::json!({ "sources": sources, "profile_id": profile_id, "options": options }),
        )
        .await?;
        Ok(id)
    }

    /// Replaces a job's send options (step 4 of the wizard). Allowed until the job is sent:
    /// in Draft, AwaitingApproval and Ready. The analysis and the decisions are kept.
    pub async fn set_options(&self, job_id: &str, options: SendOptions) -> Result<()> {
        let _job = self.begin(job_id, Op::SetOptions).await?;
        let row = self.store.get_job(job_id).await?;
        let profile = self.get_profile(&row.profile_id).await?;
        self.check_target_dir(&profile, options.target_dir.as_deref())?;
        let old: SendOptions = serde_json::from_value(row.options).unwrap_or_default();
        let retarget = old.target_dir != options.target_dir;
        let options = serde_json::to_value(&options)?;
        self.store.set_job_options(job_id, options.clone()).await?;
        self.store
            .append_event(Some(job_id), "options", options)
            .await?;
        // A new target folder moves the planned destinations (the Files tab shows them).
        if retarget && !matches!(JobState::parse(&row.state)?, JobState::Draft) {
            self.plan_destinations(job_id, &profile).await.ok();
        }
        Ok(())
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

    pub(crate) async fn summary(&self, row: JobRow) -> Result<JobSummary> {
        let counts = counts(self.store.manifest_counts(&row.id).await?);
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
            label: row.label,
            again_of: row.again_of,
        })
    }

    pub(crate) async fn job_sources(&self, row: &JobRow) -> Vec<PathBuf> {
        row.sources.iter().map(|s| bytes_path(s)).collect()
    }

    // ---- stages 2–3: analysis and decisions ---------------------------------------------

    /// Scans the sources, runs the name rules and the preflight checks. Re-running replaces
    /// the previous analysis and its decisions.
    pub async fn analyse(&self, job_id: &str) -> Result<AnalysisSummary> {
        let mut job = self.begin(job_id, Op::Analyse).await?;
        let row = self.store.get_job(job_id).await?;
        job.go(JobState::Analysing, None).await?;
        match self.analyse_inner(&mut job, &row).await {
            Ok(summary) => Ok(summary),
            Err(e) => {
                job.go(JobState::Draft, Some(e.to_string())).await?;
                Err(e)
            }
        }
    }

    async fn analyse_inner(&self, job: &mut JobLock<'_>, row: &JobRow) -> Result<AnalysisSummary> {
        let job_id = row.id.clone();
        let profile = self.get_profile(&row.profile_id).await?;
        let names = self.registry.name_plan(&profile)?;
        let sources = self.job_sources(row).await;
        let options: SendOptions = serde_json::from_value(row.options.clone()).unwrap_or_default();

        self.store.clear_issues(&job_id).await?;
        self.store.clear_manifest(&job_id).await?;

        let analysis = if JobKind::parse(&row.kind)? == JobKind::Repair {
            self.scan_destination(&job_id, &profile, options.target_dir.as_deref(), &names)
                .await?
        } else {
            let events = self.events.clone();
            let jid = job_id.clone();
            tokio::task::spawn_blocking(move || {
                let (mut files, mut bytes) = (0, 0);
                analyse::analyse_sources(&sources, &names, |src, f, b| {
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
            .map_err(|e| CoreError::Invalid(format!("analysis task failed: {e}")))??
        };

        let mut entries = analysis.entries;
        while !entries.is_empty() {
            let rest = entries.split_off(entries.len().min(INSERT_BATCH));
            self.store.insert_entries(&job_id, entries).await?;
            entries = rest;
        }
        let dest_paths = analysis
            .dest_paths
            .into_iter()
            .map(|(i, p)| {
                (
                    i,
                    p.map(|p| crate::browse::under(options.target_dir.as_deref(), p)),
                )
            })
            .collect();
        self.store.set_dest_paths(&job_id, dest_paths).await?;
        self.store.insert_issues(&job_id, analysis.issues).await?;
        if let Some(from) = &row.again_of {
            self.carry_decisions(&job_id, from).await?;
        }

        let preflight = self.preflight(&profile, &job_id).await?;
        {
            let mut it = preflight.iter().cloned();
            if let Some(transport) = it.next() {
                let test = ProfileTest {
                    transport,
                    integration: it.next(),
                };
                self.keep_test(&profile.id, &test).await;
            }
        }
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
        job.go(next, None).await?;
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
        self.rule_counts_matching(job_id, None).await
    }

    /// Per-rule counts of the issues whose source path contains `query`.
    pub async fn rule_counts_matching(
        &self,
        job_id: &str,
        query: Option<String>,
    ) -> Result<Vec<RuleCount>> {
        Ok(self
            .store
            .issue_summary_matching(job_id, query)
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
        self.issues_matching(job_id, rule, after_id, limit, None)
            .await
    }

    /// [`Self::issues`], only those whose source path contains `query`.
    pub async fn issues_matching(
        &self,
        job_id: &str,
        rule: Option<String>,
        after_id: Option<i64>,
        limit: u32,
        query: Option<String>,
    ) -> Result<Vec<IssueView>> {
        let rows = self
            .store
            .issues_page_matching(job_id, rule, after_id, limit, query)
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
            out.push(self.issue_view(r, path));
        }
        Ok(out)
    }

    /// An issue as the UI shows it; `path` is its entry's source path.
    pub(crate) fn issue_view(&self, r: IssueRow, path: String) -> IssueView {
        IssueView {
            id: r.id,
            entry_idx: r.issue.entry_idx,
            path,
            before_text: String::from_utf8_lossy(&r.issue.before).into_owned(),
            after_text: String::from_utf8_lossy(&r.issue.after).into_owned(),
            rule: r.issue.rule,
            severity: decisions::severity(&r.issue.severity),
            message_key: r.issue.message_key,
            before: r.issue.before,
            after: r.issue.after,
            decision: decisions::view(r.issue.decision),
            custom_name: r.custom_name,
            carried_over: r.carried_over,
        }
    }

    /// Records a decision; the job becomes Ready once nothing is pending. A decision the Name
    /// plan can't honour (accepting an undecodable name, keeping or typing a name a rule would
    /// still change) is refused here, not at Send.
    pub async fn decide_issue(
        &self,
        job_id: &str,
        issue_id: i64,
        input: DecisionInput,
    ) -> Result<()> {
        let mut job = self.begin(job_id, Op::Decide).await?;
        let row = self.store.issue(job_id, issue_id).await?;
        let choice = decisions::choice(input);
        let names = self.job_name_plan(job_id).await?;
        let entry = self.entry(job_id, row.issue.entry_idx).await?;
        names.check(&entry, &row.issue.rule, &choice)?;
        let (decision, name) = decisions::stored(choice);
        self.store
            .decide_issue(job_id, issue_id, decision, name)
            .await?;
        self.ready_if_decided(&mut job).await
    }

    /// Applies one decision to every pending issue of a job, or of one `rule`, in one store
    /// transaction. Issues already decided keep their decision. `Edit` is refused: a custom
    /// name belongs to one file. Refused as a whole if the decision can't be honoured for one
    /// of the issues. Returns how many issues changed.
    pub async fn decide_all(
        &self,
        job_id: &str,
        rule: Option<String>,
        input: DecisionInput,
    ) -> Result<u64> {
        self.decide_all_matching(job_id, rule, input, None).await
    }

    /// [`Self::decide_all`], only for issues whose source path contains `query`.
    pub async fn decide_all_matching(
        &self,
        job_id: &str,
        rule: Option<String>,
        input: DecisionInput,
        query: Option<String>,
    ) -> Result<u64> {
        let choice = decisions::choice(input);
        // Matches like the store's LIKE does: a substring, ASCII letters in any case.
        let needle = query
            .as_deref()
            .map(str::trim)
            .filter(|q| !q.is_empty())
            .map(|q| q.to_ascii_lowercase());
        if matches!(choice, Choice::Edit(_)) {
            return Err(CoreError::Invalid(
                "an edited name applies to one issue, not to many".into(),
            ));
        }
        let mut job = self.begin(job_id, Op::Decide).await?;
        if choice != Choice::Skip {
            let names = self.job_name_plan(job_id).await?;
            let entries = self.entries(job_id).await?;
            let pending = self
                .all_issues(job_id, rule.clone())
                .await?
                .into_iter()
                .filter(|r| r.issue.decision == Decision::Pending)
                .filter(
                    |r| match (&needle, entries.get(r.issue.entry_idx as usize)) {
                        (Some(n), Some(e)) => String::from_utf8_lossy(&e.raw_path)
                            .to_ascii_lowercase()
                            .contains(n.as_str()),
                        _ => true,
                    },
                );
            let mut refused = pending.filter_map(|r| {
                let entry = entries.get(r.issue.entry_idx as usize)?;
                names.check(entry, &r.issue.rule, &choice).err()
            });
            if let Some(first) = refused.next() {
                return Err(CoreError::Invalid(format!(
                    "{} of these issues can't take that decision; decide them one by one. \
                     First: {first}",
                    1 + refused.count()
                )));
            }
        }
        let (decision, _) = decisions::stored(choice);
        let n = self
            .store
            .decide_pending_issues_matching(job_id, rule.clone(), decision, query)
            .await?;
        if n > 0 {
            self.log(
                Some(job_id),
                "decisions",
                serde_json::json!({ "rule": rule, "decision": decision.as_str(), "count": n }),
            )
            .await?;
        }
        self.ready_if_decided(&mut job).await?;
        Ok(n)
    }

    /// One decision (not `Edit`) for each listed issue; the ones the name rules refuse keep
    /// theirs and are reported, the rest change in one transaction (docs/design/specs/D08).
    pub async fn decide_many(
        &self,
        job_id: &str,
        issue_ids: Vec<i64>,
        input: DecisionInput,
    ) -> Result<DecideManyResult> {
        const MAX: usize = 10_000;
        let choice = decisions::choice(input);
        if matches!(choice, Choice::Edit(_)) {
            return Err(CoreError::Invalid(
                "an edited name applies to one issue, not to many".into(),
            ));
        }
        if issue_ids.len() > MAX {
            return Err(CoreError::Invalid(format!(
                "at most {MAX} issues at once; select by path instead"
            )));
        }
        let mut job = self.begin(job_id, Op::Decide).await?;
        let names = self.job_name_plan(job_id).await?;
        let entries = self.entries(job_id).await?;
        let (mut ok, mut refused) = (Vec::new(), Vec::new());
        for id in issue_ids {
            let row = match self.store.issue(job_id, id).await {
                Ok(r) => r,
                Err(e) => {
                    refused.push(RefusedIssue {
                        id,
                        reason: e.to_string(),
                    });
                    continue;
                }
            };
            let check = entries
                .get(row.issue.entry_idx as usize)
                .map(|e| names.check(e, &row.issue.rule, &choice));
            match check {
                Some(Ok(())) => ok.push(id),
                Some(Err(e)) => refused.push(RefusedIssue {
                    id,
                    reason: e.to_string(),
                }),
                None => refused.push(RefusedIssue {
                    id,
                    reason: "not in the manifest".into(),
                }),
            }
        }
        let (decision, _) = decisions::stored(choice);
        let changed = self.store.decide_issues(job_id, ok, decision).await?;
        if changed > 0 {
            self.log(
                Some(job_id),
                "decisions",
                serde_json::json!({ "decision": decision.as_str(), "count": changed, "selected": true }),
            )
            .await?;
        }
        self.ready_if_decided(&mut job).await?;
        Ok(DecideManyResult { changed, refused })
    }

    async fn ready_if_decided(&self, job: &mut JobLock<'_>) -> Result<()> {
        if job.state() == JobState::AwaitingApproval
            && self.store.pending_issue_count(job.id()).await? == 0
        {
            job.go(JobState::Ready, None).await?;
        }
        Ok(())
    }

    /// Applies the decisions to the manifest: final destination paths and skipped entries.
    async fn plan_destinations(&self, job_id: &str, profile: &Profile) -> Result<()> {
        let row = self.store.get_job(job_id).await?;
        let options: SendOptions = serde_json::from_value(row.options).unwrap_or_default();
        let target = options.target_dir;
        let names = self.registry.name_plan(profile)?;
        let entries = self.entries(job_id).await?;
        let decided: Vec<_> = self
            .all_issues(job_id, None)
            .await?
            .into_iter()
            .map(decisions::decided)
            .collect();
        let planned = names.resolve(&entries, &decided)?;
        let mut dest = Vec::new();
        let mut skipped = Vec::new();
        for (i, p) in planned.into_iter().enumerate() {
            match p {
                Planned::Path(path) => dest.push((
                    i as u64,
                    Some(crate::browse::under(target.as_deref(), path)),
                )),
                Planned::Skipped => skipped.push((i as u64, EntryState::Skipped, None)),
            }
        }
        self.store.set_dest_paths(job_id, dest).await?;
        self.store.set_transfer_states(job_id, skipped).await?;
        Ok(())
    }

    pub(crate) async fn job_name_plan(&self, job_id: &str) -> Result<NamePlan> {
        let row = self.store.get_job(job_id).await?;
        let profile = self.get_profile(&row.profile_id).await?;
        self.registry.name_plan(&profile)
    }

    /// Every manifest entry of a job, in scanner order, as the name rules see it.
    pub(crate) async fn entries(&self, job_id: &str) -> Result<Vec<Entry>> {
        let mut out = Vec::new();
        let mut after = None;
        loop {
            let page = self.store.entries_page(job_id, after, PAGE).await?;
            let Some(last) = page.last() else { break };
            after = Some(last.entry.idx);
            for e in &page {
                if e.entry.idx != out.len() as u64 {
                    return Err(CoreError::Invalid("manifest has gaps".into()));
                }
                out.push(decisions::entry(e));
            }
        }
        Ok(out)
    }

    async fn entry(&self, job_id: &str, idx: u64) -> Result<Entry> {
        let page = self
            .store
            .entries_page(job_id, idx.checked_sub(1), 1)
            .await?;
        match page.first() {
            Some(e) if e.entry.idx == idx => Ok(decisions::entry(e)),
            _ => Err(CoreError::Invalid(format!(
                "entry {idx} is not in the manifest"
            ))),
        }
    }

    pub(crate) async fn all_issues(
        &self,
        job_id: &str,
        rule: Option<String>,
    ) -> Result<Vec<IssueRow>> {
        let mut out = Vec::new();
        let mut after = None;
        loop {
            let page = self
                .store
                .issues_page(job_id, rule.clone(), after, PAGE)
                .await?;
            let Some(last) = page.last() else { break };
            after = Some(last.id);
            out.extend(page);
        }
        Ok(out)
    }

    // ---- lifecycle -----------------------------------------------------------------------

    /// Sends a Ready job (or resumes a Paused one) in the background.
    pub async fn start(self: &Arc<Self>, job_id: &str) -> Result<()> {
        {
            let mut job = self.begin(job_id, Op::Start).await?;
            if job.state() == JobState::Ready {
                let row = self.store.get_job(job_id).await?;
                let profile = self.get_profile(&row.profile_id).await?;
                self.plan_destinations(job_id, &profile).await?;
            }
            job.go(JobState::Transferring, None).await?;
        }
        self.spawn_run(job_id).await;
        Ok(())
    }

    /// Same as [`Engine::start`] on a Paused job.
    pub async fn resume(self: &Arc<Self>, job_id: &str) -> Result<()> {
        self.start(job_id).await
    }

    /// Stops the runner; the job keeps its progress and can be resumed.
    pub async fn pause(&self, job_id: &str) -> Result<()> {
        // The runner takes the job's lock to record its own transitions: stop it first.
        self.stop_runner(job_id).await;
        let mut job = self.begin(job_id, Op::Pause).await?;
        job.go(JobState::Paused, None).await
    }

    /// Stops the job for good. Nothing at the source or destination is deleted.
    pub async fn cancel(&self, job_id: &str) -> Result<()> {
        self.stop_runner(job_id).await;
        let mut job = self.begin(job_id, Op::Cancel).await?;
        job.go(JobState::Cancelled, None).await
    }

    /// On startup: restarts every job that was running when the app stopped. Returns their ids.
    pub async fn resume_unfinished(self: &Arc<Self>) -> Result<Vec<String>> {
        let mut resumed = Vec::new();
        for row in self.store.list_unfinished_jobs().await? {
            let mut job = self.begin(&row.id, Op::Recover).await?;
            match job.state() {
                JobState::Analysing => {
                    job.go(
                        JobState::Draft,
                        Some("the analysis was interrupted; run it again".into()),
                    )
                    .await?;
                }
                st if st.is_running() => {
                    drop(job);
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

/// A profile change for the log: its id and name only (never the destination's host or keys).
fn profile_event(action: &str, profile: &Profile) -> serde_json::Value {
    serde_json::json!({ "action": action, "profile_id": profile.id, "name": profile.name })
}
