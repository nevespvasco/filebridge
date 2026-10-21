//! Typed events `core` emits for the UI and CLI (docs/02, "Progress events"). Progress events are
//! advisory and may be dropped by slow subscribers; state changes are also persisted to the
//! job log.

use serde::{Deserialize, Serialize};

use crate::JobState;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    JobStateChanged {
        job_id: String,
        from: JobState,
        to: JobState,
        error: Option<String>,
    },
    ScanProgress {
        job_id: String,
        files: u64,
        bytes: u64,
        current: Option<String>,
    },
    TransferProgress {
        job_id: String,
        bytes_done: u64,
        bytes_total: u64,
        files_done: u64,
        files_total: u64,
        bytes_per_sec: f64,
        eta_secs: Option<u64>,
    },
    EntryFailed {
        job_id: String,
        path: String,
        error: String,
        retryable: bool,
    },
    RetryScheduled {
        job_id: String,
        attempt: u32,
        in_secs: u64,
        error: String,
    },
    VerifyProgress {
        job_id: String,
        checked: u64,
        total: u64,
        mismatches: u64,
    },
    IntegrationProgress {
        job_id: String,
        /// i18n key, e.g. `"nextcloud.scan"`.
        step: String,
        done: u64,
        total: u64,
        items: u64,
        current: Option<String>,
        /// i18n key while the tool is busy without output (e.g. a slow disk).
        waiting: Option<String>,
    },
    IntegrationProblem {
        job_id: String,
        code: String,
        message: String,
        path: Option<String>,
    },
    JobFinished {
        job_id: String,
        state: JobState,
        error: Option<String>,
    },
}

impl Event {
    pub fn job_id(&self) -> &str {
        match self {
            Self::JobStateChanged { job_id, .. }
            | Self::ScanProgress { job_id, .. }
            | Self::TransferProgress { job_id, .. }
            | Self::EntryFailed { job_id, .. }
            | Self::RetryScheduled { job_id, .. }
            | Self::VerifyProgress { job_id, .. }
            | Self::IntegrationProgress { job_id, .. }
            | Self::IntegrationProblem { job_id, .. }
            | Self::JobFinished { job_id, .. } => job_id,
        }
    }
}
