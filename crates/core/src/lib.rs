//! The heart of filebridge: jobs, the 6-stage pipeline, the job state machine, the plugin
//! `Registry` and typed progress events. See `docs/02-architecture.md`.
//!
//! `core` only knows contracts (`transport-api`, `integration-api`, `names`); the binary that
//! hosts the engine registers concrete plugins in a [`Registry`].

mod analyse;
pub mod backoff;
mod engine;
mod event;
pub mod inhibit;
mod paths;
mod profile;
mod registry;
mod run;
mod state;

pub use analyse::{RULE_COLLISION, RULE_UNDECODABLE};
pub use engine::{
    AnalysisSummary, CheckView, Counts, DecisionInput, Engine, EngineOptions, IssueView,
    JobSummary, PreflightView, ProfileTest, RuleCount, SendOptions,
};
pub use event::Event;
pub use profile::{Family, Profile, TargetSpec};
pub use registry::Registry;
pub use state::{JobKind, JobState};

pub use filebridge_store::Store;

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
