//! One Tauri command per `filebridge_ipc::Client` method. They only forward: no business logic
//! lives in the window (ADR 0008).

use filebridge_core::{
    AnalysisSummary, DecisionInput, IssueView, JobSummary, Profile, ProfileTest, RuleCount,
    SendOptions,
};
use filebridge_ipc::{DetectNextcloudRequest, DetectNextcloudResponse, ErrorBody, Pong};
use tauri::State;

use crate::service::Service;
use crate::tray::TrayMenu;

type Res<T> = Result<T, ErrorBody>;

#[tauri::command]
#[specta::specta]
pub async fn ping(svc: State<'_, Service>) -> Res<Pong> {
    svc.client().await?.ping().await
}

#[tauri::command]
#[specta::specta]
pub async fn list_profiles(svc: State<'_, Service>) -> Res<Vec<Profile>> {
    svc.client().await?.list_profiles().await
}

#[tauri::command]
#[specta::specta]
pub async fn save_profile(svc: State<'_, Service>, profile: Profile) -> Res<Profile> {
    svc.client().await?.save_profile(profile).await
}

#[tauri::command]
#[specta::specta]
pub async fn delete_profile(svc: State<'_, Service>, id: String) -> Res<()> {
    svc.client().await?.delete_profile(id).await
}

#[tauri::command]
#[specta::specta]
pub async fn test_profile(svc: State<'_, Service>, id: String) -> Res<ProfileTest> {
    svc.client().await?.test_profile(id).await
}

#[tauri::command]
#[specta::specta]
pub async fn detect_nextcloud(
    svc: State<'_, Service>,
    req: DetectNextcloudRequest,
) -> Res<DetectNextcloudResponse> {
    svc.client().await?.detect_nextcloud(req).await
}

#[tauri::command]
#[specta::specta]
pub async fn create_job(
    svc: State<'_, Service>,
    sources: Vec<String>,
    profile_id: String,
    options: SendOptions,
) -> Res<String> {
    svc.client()
        .await?
        .create_job(sources, profile_id, options)
        .await
}

#[tauri::command]
#[specta::specta]
pub async fn analyse(svc: State<'_, Service>, job_id: String) -> Res<AnalysisSummary> {
    svc.client().await?.analyse(job_id).await
}

#[tauri::command]
#[specta::specta]
pub async fn rule_counts(svc: State<'_, Service>, job_id: String) -> Res<Vec<RuleCount>> {
    svc.client().await?.rule_counts(job_id).await
}

#[tauri::command]
#[specta::specta]
pub async fn issues(
    svc: State<'_, Service>,
    job_id: String,
    rule: Option<String>,
    after_id: Option<i64>,
    limit: u32,
) -> Res<Vec<IssueView>> {
    svc.client()
        .await?
        .issues(job_id, rule, after_id, limit)
        .await
}

#[tauri::command]
#[specta::specta]
pub async fn decide(
    svc: State<'_, Service>,
    job_id: String,
    issue_id: i64,
    decision: DecisionInput,
) -> Res<()> {
    svc.client().await?.decide(job_id, issue_id, decision).await
}

#[tauri::command]
#[specta::specta]
pub async fn start(svc: State<'_, Service>, job_id: String) -> Res<()> {
    svc.client().await?.start(job_id).await
}

#[tauri::command]
#[specta::specta]
pub async fn pause(svc: State<'_, Service>, job_id: String) -> Res<()> {
    svc.client().await?.pause(job_id).await
}

#[tauri::command]
#[specta::specta]
pub async fn resume(svc: State<'_, Service>, job_id: String) -> Res<()> {
    svc.client().await?.resume(job_id).await
}

#[tauri::command]
#[specta::specta]
pub async fn cancel(svc: State<'_, Service>, job_id: String) -> Res<()> {
    svc.client().await?.cancel(job_id).await
}

#[tauri::command]
#[specta::specta]
pub async fn list_jobs(svc: State<'_, Service>, limit: u32, offset: u32) -> Res<Vec<JobSummary>> {
    svc.client().await?.list_jobs(limit, offset).await
}

#[tauri::command]
#[specta::specta]
pub async fn job(svc: State<'_, Service>, job_id: String) -> Res<JobSummary> {
    svc.client().await?.job(job_id).await
}

/// The socket path or pipe name the window connects to (for the Details panel).
#[tauri::command]
#[specta::specta]
pub fn service_endpoint(svc: State<'_, Service>) -> String {
    svc.endpoint()
}

/// Translated tray menu labels; the strings live in the UI's i18n files.
#[tauri::command]
#[specta::specta]
pub fn set_tray_labels(tray: State<'_, TrayMenu>, show: String, quit: String) -> Res<()> {
    tray.set_labels(&show, &quit).map_err(|e| ErrorBody {
        code: "internal".into(),
        message: e.to_string(),
    })
}
