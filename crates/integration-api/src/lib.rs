//! The `Integration` contract: validate a destination tool before a send and update it after.
//! No implementation code lives here.
//!
//! An integration never touches SQLite or the keyring: `core` hands it a [`JobContext`] with
//! everything it needs. `specta::Type` derives are added together with `transport-api`'s, when
//! these types first cross into the UI.

use std::fmt;

use async_trait::async_trait;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

pub use filebridge_names::NameRule;
pub use filebridge_transport_api::{
    Check, CheckStatus, Destination, PreflightReport, RemotePath, TransportCaps, TransportError,
};

pub type Result<T, E = IntegrationError> = std::result::Result<T, E>;

/// Every integration error says whether retrying could help, so `core` picks backoff or failure.
#[derive(Debug, Clone, thiserror::Error, Serialize, Deserialize)]
#[error("{message}")]
pub struct IntegrationError {
    pub message: String,
    pub retryable: bool,
}

impl IntegrationError {
    pub fn retryable(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            retryable: true,
        }
    }

    pub fn fatal(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            retryable: false,
        }
    }
}

impl From<TransportError> for IntegrationError {
    fn from(e: TransportError) -> Self {
        Self {
            message: e.message,
            retryable: e.retryable,
        }
    }
}

/// What an integration may know about the destination profile. Secrets are not part of it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProfileView {
    pub id: String,
    pub name: String,
    pub destination: Destination,
    /// This integration's own settings, as its profile wizard saved them. Opaque to `core`.
    pub settings: serde_json::Value,
}

impl ProfileView {
    /// Decodes [`ProfileView::settings`] into the integration's settings type.
    pub fn settings_as<T: DeserializeOwned>(&self) -> Result<T> {
        serde_json::from_value(self.settings.clone()).map_err(|e| {
            IntegrationError::fatal(format!(
                "profile {:?} has invalid integration settings: {e}",
                self.name
            ))
        })
    }
}

/// Everything `core` gives an integration for one job.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobContext {
    pub job_id: String,
    pub profile: ProfileView,
    /// Capabilities of the transport used for this job.
    pub caps: TransportCaps,
}

impl JobContext {
    pub fn destination(&self) -> &Destination {
        &self.profile.destination
    }
}

/// Progress of the step currently running (e.g. folders scanned out of the total).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StepProgress {
    /// i18n key of the step, e.g. `"nextcloud.scan"`.
    pub step: String,
    /// Units done and total, e.g. folders. `total == 0` means unknown.
    pub done: u64,
    pub total: u64,
    /// Items the tool processed so far, e.g. files indexed.
    pub items: u64,
    /// What is being processed now, for display.
    pub current: Option<String>,
}

/// What [`Problem::fix`] the UI should offer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FixAction {
    /// The "Repair destination" flow (Phase 2).
    RepairDestination,
    /// Run the integration step again.
    Retry,
}

/// Something the user must see. Does not abort the step; the step's result lists it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Problem {
    /// Stable id, also the i18n key, e.g. `"nextcloud.incompatible-encoding"`.
    pub code: String,
    /// The tool's own message, untranslated, for the Details panel.
    pub message: String,
    /// Destination-relative path the problem is about, when known.
    pub path: Option<String>,
    pub retryable: bool,
    pub fix: Option<FixAction>,
}

/// Typed integration progress. Separate from `transport_api::ProgressEvent` because an
/// integration's steps (permissions, indexing) are not byte transfers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum IntegrationEvent {
    /// A step started; the value is its i18n key, e.g. `"nextcloud.permissions"`.
    Stage(String),
    Progress(StepProgress),
    /// No output for a while, but the tool is still working (e.g. a slow disk). The value is an
    /// i18n key, e.g. `"nextcloud.waiting-for-disk"`.
    Waiting(String),
    /// Also listed in [`AfterTransferReport::problems`]; sent as soon as it is seen.
    Problem(Problem),
}

/// Cloneable, cheap-to-call handle for integration events. Dropped events are fine: progress is
/// advisory, the returned report is authoritative.
#[derive(Debug, Clone, Default)]
pub struct IntegrationSink(Option<mpsc::UnboundedSender<IntegrationEvent>>);

impl IntegrationSink {
    pub fn channel() -> (Self, mpsc::UnboundedReceiver<IntegrationEvent>) {
        let (tx, rx) = mpsc::unbounded_channel();
        (Self(Some(tx)), rx)
    }

    /// A sink that discards everything.
    pub fn noop() -> Self {
        Self(None)
    }

    pub fn send(&self, event: IntegrationEvent) {
        if let Some(tx) = &self.0 {
            let _ = tx.send(event);
        }
    }
}

/// Outcome of [`Integration::after_transfer`].
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AfterTransferReport {
    /// What the tool updated, for display (e.g. the folders Nextcloud re-indexed).
    pub updated: Vec<String>,
    /// Items the tool processed (e.g. files indexed).
    pub items: u64,
    pub problems: Vec<Problem>,
}

impl AfterTransferReport {
    pub fn is_clean(&self) -> bool {
        self.problems.is_empty()
    }
}

impl fmt::Display for AfterTransferReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} updated, {} items, {} problems",
            self.updated.len(),
            self.items,
            self.problems.len()
        )
    }
}

#[async_trait]
pub trait Integration: Send + Sync {
    fn id(&self) -> &str;
    /// Checked against the transport's capabilities when a profile is created.
    fn required_caps(&self) -> TransportCaps;
    /// Name rules the tool imposes, added to the general rules for the job.
    fn name_rules(&self) -> Vec<Box<dyn NameRule>>;
    async fn preflight(&self, ctx: &JobContext) -> Result<PreflightReport>;
    /// Updates the tool for exactly the `changed` paths. Problems that don't stop the step go in
    /// the report; `Err` means the step itself failed.
    async fn after_transfer(
        &self,
        ctx: &JobContext,
        changed: &[RemotePath],
        progress: IntegrationSink,
    ) -> Result<AfterTransferReport>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use filebridge_transport_api::Backend;

    fn ctx(settings: serde_json::Value) -> JobContext {
        JobContext {
            job_id: "job-1".into(),
            profile: ProfileView {
                id: "p1".into(),
                name: "Casa".into(),
                destination: Destination {
                    transport: "rclone".into(),
                    backend: Backend::Local,
                    root: "/data".into(),
                },
                settings,
            },
            caps: TransportCaps::RESUME,
        }
    }

    #[test]
    fn settings_decode_and_errors_are_fatal() {
        #[derive(Debug, Deserialize)]
        struct S {
            user: String,
        }
        let c = ctx(serde_json::json!({ "user": "alice" }));
        assert_eq!(c.profile.settings_as::<S>().unwrap().user, "alice");
        assert_eq!(c.destination().root, "/data");

        let err = ctx(serde_json::json!({}))
            .profile
            .settings_as::<S>()
            .unwrap_err();
        assert!(!err.retryable);
        assert!(err.message.contains("Casa"));
    }

    #[test]
    fn transport_errors_keep_retryable() {
        let e: IntegrationError = TransportError::retryable("net down").into();
        assert!(e.retryable);
        assert_eq!(e.to_string(), "net down");
    }

    #[tokio::test]
    async fn sink_delivers_and_noop_discards() {
        let (sink, mut rx) = IntegrationSink::channel();
        sink.clone().send(IntegrationEvent::Stage("x".into()));
        drop(sink);
        assert_eq!(rx.recv().await, Some(IntegrationEvent::Stage("x".into())));
        assert_eq!(rx.recv().await, None);
        IntegrationSink::noop().send(IntegrationEvent::Waiting("y".into()));
    }
}
