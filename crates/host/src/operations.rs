//! The engine behind the ipc seam: every operation in the catalogue
//! (`filebridge_ipc::for_each_operation!`), run against the [`Engine`] and the plugins this host
//! wired. `filebridge-service` serves it on the socket.

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use filebridge_core::{CoreError, Engine, Event, StoreError};
use filebridge_ipc::{
    AnalysisSummary, AppSettings, DecisionInput, DetectNextcloudRequest, DetectNextcloudResponse,
    ErrorBody, Handler, IssueView, JobSummary, Operations, PROTOCOL_VERSION, Pong, Profile,
    ProfileTest, RuleCount, SendOptions,
};
use tokio::sync::broadcast;

use crate::nextcloud::NextcloudWizard;

/// Answers each operation with the engine; plugin-specific ones with the plugins.
pub struct EngineOperations {
    engine: Arc<Engine>,
    nextcloud: Option<NextcloudWizard>,
}

impl EngineOperations {
    /// Without plugin-specific operations (they answer `unsupported`).
    pub fn new(engine: Arc<Engine>) -> Self {
        Self {
            engine,
            nextcloud: None,
        }
    }

    pub(crate) fn with_nextcloud(engine: Arc<Engine>, nextcloud: Option<NextcloudWizard>) -> Self {
        Self { engine, nextcloud }
    }
}

/// How an engine error travels: a stable `code` for the UI to branch on, and a message.
fn to_error(e: CoreError) -> ErrorBody {
    let code = match &e {
        CoreError::PendingDecisions(_) => "pending_decisions",
        CoreError::InvalidTransition { .. } => "invalid_transition",
        CoreError::Store(StoreError::NotFound(_)) => "not_found",
        CoreError::Invalid(_) => "invalid",
        _ => "internal",
    };
    ErrorBody {
        code: code.into(),
        message: e.to_string(),
    }
}

type Res<T> = Result<T, ErrorBody>;

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

    async fn rule_counts(&self, job_id: String) -> Res<Vec<RuleCount>> {
        self.engine.rule_counts(&job_id).await.map_err(to_error)
    }

    async fn issues(
        &self,
        job_id: String,
        rule: Option<String>,
        after_id: Option<i64>,
        limit: u32,
    ) -> Res<Vec<IssueView>> {
        self.engine
            .issues(&job_id, rule, after_id, limit.min(1000))
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
    ) -> Res<u64> {
        self.engine
            .decide_all(&job_id, rule, decision)
            .await
            .map_err(to_error)
    }

    async fn start(&self, job_id: String) -> Res<()> {
        self.engine.start(&job_id).await.map_err(to_error)
    }

    async fn pause(&self, job_id: String) -> Res<()> {
        self.engine.pause(&job_id).await.map_err(to_error)
    }

    async fn resume(&self, job_id: String) -> Res<()> {
        self.engine.resume(&job_id).await.map_err(to_error)
    }

    async fn cancel(&self, job_id: String) -> Res<()> {
        self.engine.cancel(&job_id).await.map_err(to_error)
    }

    async fn list_jobs(&self, limit: u32, offset: u32) -> Res<Vec<JobSummary>> {
        self.engine
            .list_jobs(limit.min(1000), offset)
            .await
            .map_err(to_error)
    }

    async fn job(&self, job_id: String) -> Res<JobSummary> {
        self.engine.job(&job_id).await.map_err(to_error)
    }

    async fn get_settings(&self) -> Res<AppSettings> {
        Ok(self.engine.get_settings())
    }

    async fn save_settings(&self, settings: AppSettings) -> Res<()> {
        self.engine.save_settings(settings).await.map_err(to_error)
    }
}

impl Handler for EngineOperations {
    fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.engine.subscribe()
    }
}
