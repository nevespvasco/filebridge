//! Typed events `core` emits for the UI and CLI (docs/02, "Progress events"). Progress events are
//! advisory and may be dropped by slow subscribers; state changes are also persisted to the
//! job log.

use serde::{Deserialize, Serialize};

use crate::{DoneView, InFlightView, JobState};

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
        /// Files in flight now (at most the job's concurrency), in the order they started.
        in_flight: Vec<InFlightView>,
    },
    /// Files that arrived since the last one of these (at most 20 listed; `more` counts the
    /// rest). Advisory like every progress event, and never written to the job log.
    EntriesDone {
        job_id: String,
        entries: Vec<DoneView>,
        more: u64,
    },
    /// Files of a job's sources re-checked so far (advisory, D10).
    ReclaimCheckProgress {
        job_id: String,
        checked: u64,
        total: u64,
    },
    /// Rows of a report written so far (advisory).
    ReportProgress {
        job_id: String,
        rows: u64,
        total: u64,
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
    /// The transfer waits for the send window; `opens_at` is Unix ms.
    SendWindowWait { job_id: String, opens_at: i64 },
    /// The destination's free space, measured during the transfer (at its start, then every few
    /// minutes) when the destination reports it, against the bytes still to send. The job goes
    /// on either way; `free_bytes < left_bytes` means it may run out of space
    /// (docs/design/specs/D03).
    DestinationSpace {
        job_id: String,
        free_bytes: u64,
        left_bytes: u64,
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
            | Self::EntriesDone { job_id, .. }
            | Self::ReportProgress { job_id, .. }
            | Self::ReclaimCheckProgress { job_id, .. }
            | Self::EntryFailed { job_id, .. }
            | Self::RetryScheduled { job_id, .. }
            | Self::SendWindowWait { job_id, .. }
            | Self::DestinationSpace { job_id, .. }
            | Self::VerifyProgress { job_id, .. }
            | Self::IntegrationProgress { job_id, .. }
            | Self::IntegrationProblem { job_id, .. }
            | Self::JobFinished { job_id, .. } => job_id,
        }
    }
}
