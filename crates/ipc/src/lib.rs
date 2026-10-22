//! Window/CLI ↔ `filebridge-service` IPC over a local socket (ADR 0008).
//!
//! Wire format: newline-delimited JSON. A client sends `{"id": n, "type": "...", ...}`; the
//! server answers `{"kind": "response", "id": n, "ok": …}` or `{"kind": "response", "id": n,
//! "err": {…}}`. After a `subscribe` request the same connection also carries
//! `{"kind": "event", "event": …}` lines. Requests on one connection are handled concurrently.
//!
//! The socket is user-only: a Unix socket inside a 0700 directory
//! (`$XDG_RUNTIME_DIR/filebridge/service.sock`), or a per-user named pipe on Windows.

mod client;
mod operations;
mod server;

use std::path::PathBuf;

use interprocess::local_socket::{GenericFilePath, GenericNamespaced, Name, ToFsName, ToNsName};
use serde::{Deserialize, Serialize};

pub use client::Client;
pub use operations::{Operations, Request};
pub use server::{Handler, bind, serve};

/// The shared types (`filebridge-model`) and transport types requests, responses and events
/// carry, so clients need only this crate. It doesn't depend on `core`: a client never compiles
/// the engine (ADR 0008: the window reaches `core` only through `ipc`).
pub use filebridge_model::{
    AnalysisSummary, AppSettings, CategoryCount, CheckView, Counts, DecisionInput, Event, Family,
    IssueDecision, IssueSeverity, IssueView, JobKind, JobState, JobSummary, LogEntry, LogLevel,
    PreflightView, Profile, ProfileTest, RecentSource, RuleCount, SendOptions, SendWindow,
    TargetSpec, job_operations,
};
pub use filebridge_transport_api::{Backend, Destination, KeyringRef};

/// Bumped on incompatible protocol changes; `ping` returns it.
/// 2: `set_options` and `decide_all`.
/// 3: requests generated from the operation catalogue (`detect_nextcloud` takes `req`); an
///    issue's `decision` and `severity` are enums.
/// 4: `get_settings`/`save_settings`; the `waiting` job state and `send_window_wait` event;
///    `SendOptions` has no `bwlimit` (the limit is app-wide).
/// 5: `transfer_progress` lists the files in flight; the `entries_done` event; `job_log`.
/// 6: `job_categories`, `recent_sources`.
pub const PROTOCOL_VERSION: u32 = 6;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Envelope {
    pub id: u64,
    #[serde(flatten)]
    pub request: Request,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum Outgoing {
    Response {
        id: u64,
        #[serde(skip_serializing_if = "Option::is_none")]
        ok: Option<serde_json::Value>,
        #[serde(skip_serializing_if = "Option::is_none")]
        err: Option<ErrorBody>,
    },
    Event {
        event: Event,
    },
}

/// An error the service returns. `code` is stable (for the UI to branch on); `message` is
/// for people.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, thiserror::Error)]
#[error("{message}")]
pub struct ErrorBody {
    /// `pending_decisions`, `invalid_transition`, `not_found`, `invalid`, `unsupported`,
    /// `internal`.
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub struct Pong {
    pub protocol: u32,
    pub version: String,
}

/// Nextcloud direct-mode profile wizard (detection over SSH: a Proxmox LXC, or Nextcloud on the
/// SSH host itself).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub struct DetectNextcloudRequest {
    /// Name for the profile saved when detection succeeds.
    pub name: String,
    pub host: String,
    pub port: u16,
    pub user: String,
    pub key_file: Option<String>,
    pub known_hosts_file: Option<String>,
    pub nextcloud_user: String,
    /// Folder inside the user's files that receives sends (`""` for the root).
    pub folder: String,
    pub ctid: Option<u32>,
    pub install_dir: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub struct DetectStepView {
    pub id: String,
    /// `ok`, `warn` or `fail`.
    pub status: String,
    pub message: String,
    /// The commands the step ran, for the Details panel.
    pub commands: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type)]
pub struct DetectNextcloudResponse {
    /// The setup the last steps describe: `proxmox-lxc` (a container on the Proxmox node
    /// reached over SSH) or `host` (Nextcloud on the SSH host itself).
    #[serde(default)]
    pub detector: String,
    pub steps: Vec<DetectStepView>,
    /// Containers seen, when the choice was ambiguous.
    pub containers: Vec<String>,
    /// The saved profile, when every step passed.
    pub profile: Option<Profile>,
}

/// Where the service listens: a socket file, or a named pipe on Windows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Endpoint {
    Path(PathBuf),
    Pipe(String),
}

impl Endpoint {
    /// The per-user default.
    pub fn default_for_user() -> Self {
        if cfg!(windows) {
            let user = std::env::var("USERNAME").unwrap_or_else(|_| "user".into());
            return Self::Pipe(format!("filebridge-{user}"));
        }
        let dir = std::env::var_os("XDG_RUNTIME_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                let user = std::env::var("USER").unwrap_or_else(|_| "user".into());
                std::env::temp_dir().join(format!("filebridge-{user}"))
            });
        Self::Path(dir.join("filebridge").join("service.sock"))
    }

    pub(crate) fn name(&self) -> std::io::Result<Name<'static>> {
        match self {
            Self::Path(p) => p.clone().to_fs_name::<GenericFilePath>(),
            Self::Pipe(n) => n.clone().to_ns_name::<GenericNamespaced>(),
        }
    }

    pub fn display(&self) -> String {
        match self {
            Self::Path(p) => p.display().to_string(),
            Self::Pipe(n) => format!(r"\\.\pipe\{n}"),
        }
    }
}
