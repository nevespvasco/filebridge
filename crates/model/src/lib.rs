//! The words the UI and the service share: profiles, jobs and their states, issues, events, and
//! the job state rules. Pure data, no I/O. `core` produces and enforces them; `ipc` carries them,
//! so a client compiles this crate and not the engine.
//!
//! Every type here crosses into TypeScript (`specta::Type`): renaming a field or a variant is a
//! protocol change (`filebridge_ipc::PROTOCOL_VERSION`).

mod event;
mod profile;
mod rules;
mod settings;
mod state;
mod views;

pub use event::Event;
pub use profile::{Family, Profile, TargetSpec};
pub use rules::{Op, can_go, job_operations};
pub use settings::{AppSettings, MIN_BANDWIDTH_LIMIT, SendWindow, WindowStatus};
pub use state::{JobKind, JobState};
pub use views::{
    AnalysisSummary, CheckView, Counts, DecisionInput, IssueDecision, IssueSeverity, IssueView,
    JobSummary, PreflightView, ProfileTest, RuleCount, SendOptions,
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
