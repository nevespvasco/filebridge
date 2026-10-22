//! What the UI is shown about jobs, analyses, issues and profile tests.

use filebridge_transport_api::{CheckStatus, PreflightReport};
use serde::{Deserialize, Serialize};

use crate::{JobKind, JobState};

/// Per-job choices made in step 4 of the wizard.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type)]
pub struct SendOptions {
    /// Compare SHA-256 of every file at the end (source vs destination).
    pub verify: bool,
    /// Files in flight at once.
    pub concurrency: u32,
}

impl Default for SendOptions {
    fn default() -> Self {
        Self {
            verify: false,
            concurrency: 8,
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
    pub fn from_report(source: &str, r: PreflightReport) -> Self {
        Self {
            source: source.into(),
            free_bytes: r.free_bytes,
            writable: r.writable,
            checks: r
                .checks
                .into_iter()
                .map(|c| CheckView {
                    id: c.id,
                    status: Self::status_text(c.status).into(),
                    message: c.message,
                })
                .collect(),
        }
    }

    /// `ok`, `warn` or `fail`: how a check's status travels to the UI.
    pub fn status_text(status: CheckStatus) -> &'static str {
        match status {
            CheckStatus::Ok => "ok",
            CheckStatus::Warn => "warn",
            CheckStatus::Fail => "fail",
        }
    }

    /// A preflight that could not run at all.
    pub fn failed(source: &str, message: String) -> Self {
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
    pub severity: IssueSeverity,
    pub message_key: String,
    pub before: Vec<u8>,
    pub after: Vec<u8>,
    pub before_text: String,
    pub after_text: String,
    pub decision: IssueDecision,
    pub custom_name: Option<String>,
}

/// Whether an issue's fix is applied unless overridden, or waits for the user.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum IssueSeverity {
    Auto,
    NeedsDecision,
}

/// Where the decision on an issue stands, as the UI shows it. `rejected`: keep the original name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum IssueDecision {
    Pending,
    Accepted,
    Edited,
    Rejected,
    Skipped,
}

impl IssueDecision {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Accepted => "accepted",
            Self::Edited => "edited",
            Self::Rejected => "rejected",
            Self::Skipped => "skipped",
        }
    }
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
