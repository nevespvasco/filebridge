//! The words the UI and the service share: profiles, jobs and their states, issues, events, and
//! the job state rules. Pure data, no I/O. `core` produces and enforces them; `ipc` carries them,
//! so a client compiles this crate and not the engine.
//!
//! Every type here crosses into TypeScript (`specta::Type`): renaming a field or a variant is a
//! protocol change (`filebridge_ipc::PROTOCOL_VERSION`).

mod category;
mod entries;
mod event;
mod log;
mod profile;
mod rules;
mod settings;
mod state;
mod views;

pub use category::FileCategory;
pub use entries::{
    EntryCursor, EntryDetail, EntryFacets, EntryFilter, EntryPage, EntrySort, EntryStatus,
    EntryView, ExportResult, HashView, ReportFormat, ReportRequest, ReportScope, StatusCount,
};
pub use event::Event;
pub use log::{ActivityItem, LogEntry, LogLevel, default_label, log_level};
pub use profile::{Family, Profile, TargetSpec};
pub use rules::{JobGroup, Op, can_go, job_operations};
pub use settings::{AppSettings, MIN_BANDWIDTH_LIMIT, SendWindow, WindowStatus};
pub use state::{JobKind, JobState};
pub use views::{
    AnalysisSummary, BlockerView, BrowseResult, BrowseTarget, CategoryCount, CheckView, Counts,
    DecideManyResult, DecisionInput, DoneView, FolderNameCheck, GroupCount, InFlightView,
    IssueDecision, IssueSeverity, IssueView, JobFilter, JobPage, JobSort, JobSummary,
    LOW_SPACE_BYTES, LOW_SPACE_RATIO, LocalTargetProbe, PreflightView, ProfileTest,
    RECLAIM_CHECK_FRESH_MS, RecentSource, ReclaimBlocker, ReclaimStatus, RefusedIssue,
    RemoteEntryView, RuleCount, SOURCE_CHILDREN_CAP, STORAGE_STALE_AFTER_MS, SendOptions,
    SourceInfo, SourceProblem, SourceReclaim, SpaceView, StorageView,
};

/// A stored value (a job state, a job kind) this build doesn't know.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("unknown {what} {value:?}")]
pub struct UnknownValue {
    what: &'static str,
    value: String,
}

impl UnknownValue {
    pub(crate) fn new(what: &'static str, value: &str) -> Self {
        Self {
            what,
            value: value.to_owned(),
        }
    }
}
