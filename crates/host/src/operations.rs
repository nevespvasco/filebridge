//! The engine behind the ipc seam: every operation in the catalogue
//! (`filebridge_ipc::for_each_operation!`), run against the [`Engine`] and the plugins this host
//! wired. `filebridge-service` serves it on the socket.

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use filebridge_core::{CoreError, Engine, Event, StoreError};
use filebridge_integration_nextcloud::ShellConnector;
use filebridge_ipc::{
    AccessReport, ActivityItem, AnalysisSummary, AppSettings, BrowseResult, BrowseTarget,
    CategoryCount, DecideManyResult, DecisionInput, DetectNextcloudRequest,
    DetectNextcloudResponse, EntryCursor, EntryDetail, EntryFacets, EntryFilter, EntryPage,
    ErrorBody, ExportResult, FolderNameCheck, Handler, HostKeyView, ImmichRequest, IssueView,
    JobFilter, JobPage, JobSummary, LogEntry, NextcloudWebdavRequest, Operations, PROTOCOL_VERSION,
    Pong, Profile, ProfileTest, RecentSource, ReclaimStatus, ReportRequest, RuleCount, SendOptions,
    SourceInfo, StorageView,
};
use tokio::sync::broadcast;

use crate::immich::ImmichWizard;
use crate::nextcloud::NextcloudWizard;

/// Answers each operation with the engine; plugin-specific ones with the plugins.
pub struct EngineOperations {
    engine: Arc<Engine>,
    nextcloud: Option<NextcloudWizard>,
    immich: Option<ImmichWizard>,
    /// For the SSH-key operations (ADR 0012).
    ssh: Option<Arc<dyn ShellConnector>>,
}

impl EngineOperations {
    /// Without plugin-specific operations (they answer `unsupported`).
    pub fn new(engine: Arc<Engine>) -> Self {
        Self {
            engine,
            nextcloud: None,
            immich: None,
            ssh: None,
        }
    }

    pub(crate) fn with_plugins(
        engine: Arc<Engine>,
        nextcloud: Option<NextcloudWizard>,
        immich: Option<ImmichWizard>,
        ssh: Option<Arc<dyn ShellConnector>>,
    ) -> Self {
        Self {
            engine,
            nextcloud,
            immich,
            ssh,
        }
    }

    fn ssh(&self) -> Res<&dyn ShellConnector> {
        self.ssh.as_deref().ok_or_else(|| ErrorBody {
            code: "unsupported".into(),
            message: "this service can't open SSH connections".into(),
        })
    }
}

/// How an engine error travels: a stable `code` for the UI to branch on, and a message.
fn to_error(e: CoreError) -> ErrorBody {
    let code = match &e {
        CoreError::PendingDecisions(_) => "pending_decisions",
        CoreError::InvalidTransition { .. } => "invalid_transition",
        CoreError::Store(StoreError::NotFound(_)) => "not_found",
        CoreError::Store(StoreError::Deleted(_)) => "invalid",
        CoreError::Invalid(_) => "invalid",
        _ => "internal",
    };
    ErrorBody {
        code: code.into(),
        message: e.to_string(),
    }
}

type Res<T> = Result<T, ErrorBody>;

/// The most sources one `inspect_sources` call checks.
const MAX_INSPECT: usize = 10_000;

#[async_trait]
impl Operations for EngineOperations {
    async fn ping(&self) -> Res<Pong> {
        Ok(Pong {
            protocol: PROTOCOL_VERSION,
            version: env!("CARGO_PKG_VERSION").into(),
        })
    }

    async fn list_profiles(&self) -> Res<Vec<Profile>> {
        self.engine.list_profiles().await.map_err(to_error)
    }

    async fn save_profile(&self, profile: Profile) -> Res<Profile> {
        self.engine.save_profile(profile).await.map_err(to_error)
    }

    async fn delete_profile(&self, id: String) -> Res<()> {
        self.engine.delete_profile(&id).await.map_err(to_error)
    }

    async fn last_target_dir(&self, profile_id: String) -> Res<Option<String>> {
        self.engine
            .last_target_dir(&profile_id)
            .await
            .map_err(to_error)
    }

    async fn test_profile(&self, id: String) -> Res<ProfileTest> {
        self.engine.test_profile(&id).await.map_err(to_error)
    }

    async fn detect_nextcloud(&self, req: DetectNextcloudRequest) -> Res<DetectNextcloudResponse> {
        match &self.nextcloud {
            Some(wizard) => wizard.run(&self.engine, req, None).await,
            None => Err(ErrorBody {
                code: "unsupported".into(),
                message: "this service has no Nextcloud support".into(),
            }),
        }
    }

    async fn add_nextcloud_webdav(
        &self,
        req: NextcloudWebdavRequest,
    ) -> Res<DetectNextcloudResponse> {
        match &self.nextcloud {
            Some(wizard) => wizard.webdav(&self.engine, req).await,
            None => Err(ErrorBody {
                code: "unsupported".into(),
                message: "this service has no Nextcloud support".into(),
            }),
        }
    }

    async fn add_immich(&self, req: ImmichRequest) -> Res<DetectNextcloudResponse> {
        match &self.immich {
            Some(wizard) => wizard.run(&self.engine, req).await,
            None => Err(ErrorBody {
                code: "unsupported".into(),
                message: "this service has no Immich support".into(),
            }),
        }
    }

    async fn create_job(
        &self,
        sources: Vec<String>,
        profile_id: String,
        options: SendOptions,
    ) -> Res<String> {
        let sources = sources.into_iter().map(PathBuf::from).collect();
        self.engine
            .create_job(sources, &profile_id, options)
            .await
            .map_err(to_error)
    }

    async fn set_options(&self, job_id: String, options: SendOptions) -> Res<()> {
        self.engine
            .set_options(&job_id, options)
            .await
            .map_err(to_error)
    }

    async fn analyse(&self, job_id: String) -> Res<AnalysisSummary> {
        self.engine.analyse(&job_id).await.map_err(to_error)
    }

    async fn rule_counts(&self, job_id: String, query: Option<String>) -> Res<Vec<RuleCount>> {
        self.engine
            .rule_counts_matching(&job_id, query)
            .await
            .map_err(to_error)
    }

    async fn issues(
        &self,
        job_id: String,
        rule: Option<String>,
        after_id: Option<i64>,
        limit: u32,
        query: Option<String>,
    ) -> Res<Vec<IssueView>> {
        self.engine
            .issues_matching(&job_id, rule, after_id, limit.min(1000), query)
            .await
            .map_err(to_error)
    }

    async fn decide(&self, job_id: String, issue_id: i64, decision: DecisionInput) -> Res<()> {
        self.engine
            .decide_issue(&job_id, issue_id, decision)
            .await
            .map_err(to_error)
    }

    async fn decide_all(
        &self,
        job_id: String,
        rule: Option<String>,
        decision: DecisionInput,
        query: Option<String>,
    ) -> Res<u64> {
        self.engine
            .decide_all_matching(&job_id, rule, decision, query)
            .await
            .map_err(to_error)
    }

    async fn decide_many(
        &self,
        job_id: String,
        issue_ids: Vec<i64>,
        decision: DecisionInput,
    ) -> Res<DecideManyResult> {
        self.engine
            .decide_many(&job_id, issue_ids, decision)
            .await
            .map_err(to_error)
    }

    async fn start(&self, job_id: String) -> Res<()> {
        self.engine.start(&job_id).await.map_err(to_error)
    }

    async fn pause(&self, job_id: String) -> Res<()> {
        self.engine.pause(&job_id).await.map_err(to_error)
    }

    async fn skip_entry(&self, job_id: String, idx: u64) -> Res<()> {
        self.engine.skip_entry(&job_id, idx).await.map_err(to_error)
    }

    async fn resume(&self, job_id: String) -> Res<()> {
        self.engine.resume(&job_id).await.map_err(to_error)
    }

    async fn cancel(&self, job_id: String) -> Res<()> {
        self.engine.cancel(&job_id).await.map_err(to_error)
    }

    async fn list_jobs(&self, filter: JobFilter, limit: u32, offset: u32) -> Res<JobPage> {
        self.engine
            .list_jobs_page(filter, limit.min(1000), offset)
            .await
            .map_err(to_error)
    }

    async fn set_label(&self, job_id: String, label: Option<String>) -> Res<()> {
        self.engine
            .set_label(&job_id, label)
            .await
            .map_err(to_error)
    }

    async fn send_again(&self, job_id: String, profile_id: Option<String>) -> Res<String> {
        self.engine
            .send_again(&job_id, profile_id)
            .await
            .map_err(to_error)
    }

    async fn job(&self, job_id: String) -> Res<JobSummary> {
        self.engine.job(&job_id).await.map_err(to_error)
    }

    async fn browse(
        &self,
        target: BrowseTarget,
        dir: Option<String>,
        prefix: Option<String>,
    ) -> Res<BrowseResult> {
        self.engine
            .browse(target, dir, prefix)
            .await
            .map_err(to_error)
    }

    async fn check_folder_name(&self, target: BrowseTarget, name: String) -> Res<FolderNameCheck> {
        self.engine
            .check_folder_name(target, name)
            .await
            .map_err(to_error)
    }

    async fn profile_storage(&self, id: String, refresh: bool) -> Res<StorageView> {
        self.engine
            .profile_storage(&id, refresh)
            .await
            .map_err(to_error)
    }

    async fn entries(
        &self,
        job_id: String,
        filter: EntryFilter,
        after: Option<EntryCursor>,
        limit: u32,
    ) -> Res<EntryPage> {
        self.engine
            .files_page(&job_id, filter, after, limit)
            .await
            .map_err(to_error)
    }

    async fn entry_facets(&self, job_id: String) -> Res<EntryFacets> {
        self.engine.entry_facets(&job_id).await.map_err(to_error)
    }

    async fn entry(&self, job_id: String, idx: u64) -> Res<EntryDetail> {
        self.engine
            .entry_detail(&job_id, idx)
            .await
            .map_err(to_error)
    }

    async fn reclaim_status(&self, job_id: String) -> Res<ReclaimStatus> {
        self.engine.reclaim_status(&job_id).await.map_err(to_error)
    }

    async fn recheck_sources(&self, job_id: String) -> Res<ReclaimStatus> {
        self.engine.recheck_sources(&job_id).await.map_err(to_error)
    }

    async fn dismiss_reclaim(&self, job_id: String, dismissed: bool) -> Res<()> {
        self.engine
            .dismiss_reclaim(&job_id, dismissed)
            .await
            .map_err(to_error)
    }

    async fn export_report(&self, job_id: String, req: ReportRequest) -> Res<ExportResult> {
        self.engine
            .export_report(&job_id, req)
            .await
            .map_err(to_error)
    }

    async fn activity(&self, before_id: Option<i64>, limit: u32) -> Res<Vec<ActivityItem>> {
        self.engine
            .activity(before_id, limit)
            .await
            .map_err(to_error)
    }

    async fn job_categories(&self, job_id: String) -> Res<Vec<CategoryCount>> {
        self.engine.job_categories(&job_id).await.map_err(to_error)
    }

    async fn recent_sources(&self, limit: u32) -> Res<Vec<RecentSource>> {
        self.engine
            .recent_sources(limit.min(50))
            .await
            .map_err(to_error)
    }

    async fn install_app_key(&self, profile_id: String) -> Res<AccessReport> {
        crate::access::install(&self.engine, self.ssh()?, &profile_id).await
    }

    async fn remove_access(&self, profile_id: String) -> Res<AccessReport> {
        crate::access::remove(&self.engine, self.ssh()?, &profile_id).await
    }

    async fn scan_host_key(
        &self,
        host: String,
        port: u16,
        known_hosts_file: Option<String>,
    ) -> Res<HostKeyView> {
        self.ssh()?;
        crate::host_keys::scan(host, port, known_hosts_file).await
    }

    async fn trust_host_key(
        &self,
        host: String,
        port: u16,
        known_hosts_file: Option<String>,
        fingerprint: String,
    ) -> Res<HostKeyView> {
        self.ssh()?;
        crate::host_keys::trust(host, port, known_hosts_file, fingerprint).await
    }

    async fn set_profile_password(
        &self,
        profile_id: String,
        password: Option<filebridge_ipc::Password>,
    ) -> Res<Profile> {
        self.engine
            .set_profile_password(
                &profile_id,
                password.map(filebridge_ipc::Password::into_inner),
            )
            .await
            .map_err(to_error)
    }

    async fn create_repair_job(&self, profile_id: String, dir: Option<String>) -> Res<String> {
        self.engine
            .create_repair_job(&profile_id, dir)
            .await
            .map_err(to_error)
    }

    async fn planned_renames(&self, job_id: String) -> Res<u64> {
        self.engine.planned_renames(&job_id).await.map_err(to_error)
    }

    async fn inspect_sources(&self, paths: Vec<String>) -> Res<Vec<SourceInfo>> {
        if paths.len() > MAX_INSPECT {
            return Err(ErrorBody {
                code: "invalid".into(),
                message: format!("at most {MAX_INSPECT} sources at a time"),
            });
        }
        self.engine.inspect_sources(paths).await.map_err(to_error)
    }

    async fn job_log(
        &self,
        job_id: String,
        after_id: Option<i64>,
        before_id: Option<i64>,
        limit: u32,
    ) -> Res<Vec<LogEntry>> {
        self.engine
            .job_log(&job_id, after_id, before_id, limit.min(500))
            .await
            .map_err(to_error)
    }

    async fn get_settings(&self) -> Res<AppSettings> {
        Ok(self.engine.get_settings())
    }

    async fn save_settings(&self, settings: AppSettings) -> Res<()> {
        self.engine.save_settings(settings).await.map_err(to_error)
    }

    async fn service_log(&self) -> Res<Vec<String>> {
        Ok(crate::RecentLog::global().lines())
    }
}

impl Handler for EngineOperations {
    fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.engine.subscribe()
    }
}
