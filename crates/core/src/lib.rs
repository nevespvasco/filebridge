//! The heart of filebridge: jobs, the 6-stage pipeline, the job state machine, the plugin
//! `Registry` and typed progress events. See `docs/02-architecture.md`.
//!
//! `core` only knows contracts (`transport-api`, `integration-api`, `names`); the binary that
//! hosts the engine registers concrete plugins in a [`Registry`].

mod analyse;
pub mod backoff;
mod decisions;
mod engine;
pub mod inhibit;
mod lifecycle;
mod profile;
mod registry;
mod run;

pub use engine::{Engine, EngineOptions};
/// The shared types and rules, so `core`'s callers keep their paths.
pub use filebridge_model::{
    AnalysisSummary, CheckView, Counts, DecisionInput, Event, Family, IssueDecision, IssueSeverity,
    IssueView, JobKind, JobState, JobSummary, PreflightView, Profile, ProfileTest, RuleCount,
    SendOptions, TargetSpec, UnknownValue, job_operations,
};
pub use registry::Registry;

pub use filebridge_store::{Store, StoreError};

pub type Result<T, E = CoreError> = std::result::Result<T, E>;

#[derive(Debug, thiserror::Error)]
pub enum CoreError {
    #[error(transparent)]
    Store(#[from] filebridge_store::StoreError),
    #[error("transport: {}", .0.message)]
    Transport(#[from] filebridge_transport_api::TransportError),
    #[error("integration: {}", .0.message)]
    Integration(#[from] filebridge_integration_api::IntegrationError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error("cannot go from {} to {}", .from.as_str(), .to.as_str())]
    InvalidTransition { from: JobState, to: JobState },
    #[error("{0} name decisions are still pending")]
    PendingDecisions(u64),
    #[error("{0}")]
    Invalid(String),
}

impl From<UnknownValue> for CoreError {
    fn from(e: UnknownValue) -> Self {
        CoreError::Invalid(e.to_string())
    }
}
