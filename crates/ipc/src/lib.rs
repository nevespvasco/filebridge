//! Window/CLI ↔ `filebridge-service` IPC over a local socket (ADR 0008).
//!
//! Wire format: newline-delimited JSON. A client sends `{"id": n, "request": {"type": "...", ...}}`; the
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
    ActivityItem, AnalysisSummary, AppSettings, BlockerView, BrowseResult, BrowseTarget,
    CategoryCount, CheckView, Counts, DecideManyResult, DecisionInput, EntryCursor, EntryDetail,
    EntryFacets, EntryFilter, EntryPage, EntrySort, EntryStatus, EntryThumbnail, EntryView, Event,
    ExportResult, FOLDER_SIZES_LISTED, Family, FolderNameCheck, FolderSize, FolderSizes, HashView,
    IssueDecision, IssueSeverity, IssueView, JobFilter, JobGroup, JobKind, JobPage, JobSort,
    JobState, JobSummary, LogEntry, LogLevel, MAX_PINNED_SOURCES, PinnedSource, PreflightView,
    Profile, ProfileTest, RecentSource, ReclaimBlocker, ReclaimStatus, RefusedIssue,
    RemoteEntryView, ReportChange, ReportChangeCount, ReportChangeKind, ReportCheck, ReportDiff,
    ReportFormat, ReportRequest, ReportScope, ReportSide, RuleCount, SOURCE_CHILDREN_CAP,
    SealProblem, SendOptions, SendWindow, SourceInfo, SourceProblem, SourceReclaim, SpaceView,
    StatusCount, StorageView, TargetSpec, VolumeView, job_operations,
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
/// 7: `entries`, `entry_facets`, `entry`.
/// 8: `profile_storage`; preflight views carry total and used space.
/// 9: `list_jobs` takes a filter and answers a page; `set_label`, `send_again`; jobs have a label.
/// 10: `activity`.
/// 11: `decide_many`; `issues`, `rule_counts` and `decide_all` take a path `query`.
/// 12: `export_report` and the `report_progress` event.
/// 13: `reclaim_status`, `recheck_sources`, `dismiss_reclaim`, the `reclaim_check_progress` event.
/// 14: `browse`, `check_folder_name`; `SendOptions.target_dir`.
/// 15: `inspect_sources`; nested or repeated sources refused by `create_job`.
/// 16: `create_repair_job` (`JobKind::Repair`); `TargetSpec` limits; rules `mojibake`, `length`.
/// 17: `set_profile_password` (SFTP password in the OS keyring).
/// 18: `add_nextcloud_webdav`; `Backend::Webdav`; Nextcloud settings `mode: "webdav"`.
/// 19: `install_app_key`, `remove_access`; `Backend::Sftp.key`, `SshTarget.key` (ADR 0012).
/// 20: `planned_renames`; `RuleCount.no_proposal`; `set_options` refused for Repair jobs.
/// 21: `scan_host_key`, `trust_host_key` (ADR 0015).
/// 22: the request goes under `request` beside the call's `id`, so an argument named `id`
///     (`test_profile`, `profile_storage`, `delete_profile`) no longer collides with it.
/// 23: `Event::DestinationSpace`; `JobSummary.error_code` (`destination_full`: a full
///     destination pauses the job).
/// 24: `add_immich` (ADR 0020).
/// 25: `service_log` (D07).
/// 26: `last_target_dir` (D11).
/// 27: `skip_entry` (D04).
/// 28: `Backend::Smb`, `Backend::S3` (ADR 0022).
/// 29: `pinned_sources`, `pin_source`, `volumes`, `folder_sizes`, `cancel_folder_sizes`, the
///     `folder_sizes_progress` event; `JobErrorCode::SourceUnavailable` (a source that can't be
///     read pauses the job).
/// 30: `entry_thumbnail` (ADR 0027).
/// 31: `diff_reports`, `check_report`; `ReportFormat::Sealed`, `ExportResult.readable`/`seal`
///     (ADR 0025).
pub const PROTOCOL_VERSION: u32 = 31;

/// One call: its `id` and the request, kept apart so no argument can collide with the `id`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Envelope {
    pub id: u64,
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

/// What installing filebridge's own SSH key, or removing access, did (ADR 0012): each step
/// ✓/!/✗, and the profile as saved when it went through.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type)]
pub struct AccessReport {
    pub steps: Vec<DetectStepView>,
    pub profile: Option<Profile>,
}

/// Where an SSH server's host key stands in the `known_hosts` file filebridge uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum HostKeyStatus {
    /// The file holds this key for the host: connections go through.
    Known,
    /// The file holds no key for the host: the user may trust this one (`trust_host_key`).
    Unknown,
    /// The file holds another key for the host: the server changed or the connection is being
    /// intercepted. Never trusted from the app.
    Changed,
}

/// An SSH server's host key, as `scan_host_key` and `trust_host_key` saw it (ADR 0015).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub struct HostKeyView {
    pub host: String,
    pub port: u16,
    /// The file checked, and the one `trust_host_key` adds to.
    pub known_hosts_file: String,
    pub status: HostKeyStatus,
    /// The key type the server presented, e.g. `ssh-ed25519`. `None` only when `changed` and the
    /// server offered none of the key types on record.
    pub key_type: Option<String>,
    /// `SHA256:…`, as `ssh` shows it; what the user compares and confirms.
    pub fingerprint: Option<String>,
    /// For `changed`: the line of `known_hosts_file` with the key on record.
    pub line: Option<u32>,
}

/// A password on its way to the OS keyring. On the wire it is a plain string; its `Debug` never
/// shows it, so neither does any request, envelope or log line that carries one.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(transparent)]
pub struct Password(String);

impl Password {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// The password itself, for the keyring.
    pub fn expose(&self) -> &str {
        &self.0
    }

    pub fn into_inner(self) -> String {
        self.0
    }
}

impl std::fmt::Debug for Password {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("***")
    }
}

/// Nextcloud WebDAV-mode profile wizard (docs/05): the server's address, the user and an app
/// password, which goes to the OS keyring and nowhere else.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub struct NextcloudWebdavRequest {
    /// Name for the profile saved when every check passes.
    pub name: String,
    /// What the user typed: `cloud.example.com`, or a WebDAV address copied from Nextcloud
    /// (whose `…/dav/files/<id>/` gives the user's id when the login is an email).
    pub server: String,
    /// The login the app password belongs to.
    pub user: String,
    /// A Nextcloud app password (Settings → Security), never the account password.
    pub app_password: Password,
    /// Folder inside the user's files that receives sends (`""` for the root).
    pub folder: String,
}

/// Immich profile wizard (ADR 0020): files go by `destination` (SFTP, or a local folder) into a
/// folder of an Immich external library, which is scanned after each send. The API key goes to
/// the OS keyring and nowhere else.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub struct ImmichRequest {
    /// Name for the profile saved when every check passes.
    pub name: String,
    /// Where the files go: an SFTP server or a local folder, with its root folder. Its
    /// `password`, if any, comes from [`ImmichRequest::password`].
    pub destination: Destination,
    /// The SFTP password, if the server signs in with one (kept in the keyring too).
    pub password: Option<Password>,
    /// What the user typed: `photos.example.com` (https), or an address ending in `/api`.
    pub server: String,
    /// An API key of an Immich administrator (Account settings → API keys), with at least
    /// `library.read` and `library.update`.
    pub api_key: Password,
    /// The external library; `None` picks the one that imports `import_path`.
    pub library_id: Option<String>,
    /// The destination's folder as Immich sees it (e.g. `/mnt/photos` in its container);
    /// `None` means the same path as the destination's root.
    pub import_path: Option<String>,
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
    /// Containers (Nextcloud) or libraries (Immich) seen, when the choice was ambiguous.
    pub containers: Vec<String>,
    /// The saved profile, when every step passed.
    pub profile: Option<Profile>,
}

/// The per-user entry that starts `filebridge-service` at login, as `filebridge-service
/// show-user-unit --json` prints it: the `systemd --user` unit (Linux), a LaunchAgent (macOS) or
/// a Run key value (Windows), see [`LoginEntry`]. The window reads it for the Settings switch
/// "Start the engine when I log in" and shows it before changing anything. Not an ipc request:
/// the window runs the service binary, so it works with no service running.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub struct UserUnit {
    /// Which kind of entry this is.
    pub kind: LoginEntry,
    /// Where it lives: the unit file (`~/.config/systemd/user/filebridge-service.service`), the
    /// plist (`~/Library/LaunchAgents/app.filebridge.service.plist`) or the registry value
    /// (`HKEY_CURRENT_USER\…\Run\filebridge-service`).
    pub path: String,
    /// The program the entry runs, or would run once installed: this service binary, or on
    /// Windows the app that starts it without a console window.
    pub exec: String,
    /// The entry is there.
    pub installed: bool,
    /// The OS starts it at login: `systemctl --user is-enabled`, not disabled in launchd, not
    /// turned off in Task Manager's Startup apps.
    pub enabled: bool,
    /// The entry there is the one installing would write now. `false` when it runs another
    /// binary (an earlier install from elsewhere) or other options.
    pub current: bool,
    /// The OS's login manager answers (`systemctl --user`; `launchctl` in a desktop session;
    /// always on Windows): without it nothing can be installed.
    pub manager: bool,
    /// What installing writes: the unit file, the plist, or the Run value's command line.
    pub contents: String,
    /// Whether the engine the entry starts can reach the SSH agent this program sees.
    pub agent: UnitAgent,
}

/// The argument the app takes as "start `filebridge-service` without a window, then exit". The
/// Windows Run value starts the app with it (`filebridge-service install-user-unit --launcher`),
/// because the service is a console program and would keep a console window open.
pub const START_ENGINE_ARG: &str = "--start-engine";

/// The mechanism each desktop OS starts a user's programs from at login.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum LoginEntry {
    /// Linux: a `systemd --user` unit, enabled for `default.target`.
    #[default]
    SystemdUnit,
    /// macOS: a LaunchAgent plist in `~/Library/LaunchAgents`, loaded by launchd at login.
    LaunchAgent,
    /// Windows: a value under `HKEY_CURRENT_USER\Software\Microsoft\Windows\CurrentVersion\Run`.
    RunKey,
}

/// How the engine started by the login entry finds the SSH agent (`SSH_AUTH_SOCK`), which SFTP
/// and Nextcloud destinations without filebridge's own key, a key file or a password sign in
/// with. The entry's process starts with the login manager's environment (systemd's user
/// manager, launchd), not the session's.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum UnitAgent {
    /// No SSH agent here: nothing to pass on.
    None,
    /// The agent's socket is at a fixed per-user path (under the runtime directory or home): the
    /// entry sets `SSH_AUTH_SOCK` to it.
    Pinned,
    /// The login manager's environment has `SSH_AUTH_SOCK` (systemd's user manager; launchd,
    /// which runs macOS's own agent; on Windows the entry starts with the session's
    /// environment): the entry's process inherits it.
    Manager,
    /// The agent's socket changes at each login and the login manager doesn't have it: SSH
    /// destinations that sign in with the agent fail under the entry.
    Missing,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_folder_measurement_on_the_wire() {
        let call = Envelope {
            id: 3,
            request: Request::FolderSizes {
                scan_id: "w-1".into(),
                path: "/home/demo/Fotos".into(),
            },
        };
        let wire = serde_json::to_value(&call).unwrap();
        assert_eq!(
            wire,
            serde_json::json!({ "id": 3, "request": {
                "type": "folder_sizes", "scan_id": "w-1", "path": "/home/demo/Fotos"
            }})
        );
        // Its progress isn't about a job.
        let event = Event::FolderSizesProgress {
            scan_id: "w-1".into(),
            sizes: FolderSizes {
                path: "/home/demo/Fotos".into(),
                items: Vec::new(),
                rest_items: 0,
                rest_files: 0,
                rest_bytes: 0,
                files: 0,
                dirs: 0,
                bytes: 0,
                errors: 0,
                complete: false,
            },
        };
        assert_eq!(event.job_id(), None);
        let wire = serde_json::to_value(Outgoing::Event { event }).unwrap();
        assert_eq!(wire["event"]["type"], "folder_sizes_progress");
        assert_eq!(wire["event"]["scan_id"], "w-1");
    }

    #[test]
    fn no_request_prints_a_password() {
        let requests = [
            Request::SetProfilePassword {
                profile_id: "p-1".into(),
                password: Some(Password::new("s3cret-sftp-pass")),
            },
            Request::AddNextcloudWebdav {
                req: NextcloudWebdavRequest {
                    name: "Cloud".into(),
                    server: "cloud.example.com".into(),
                    user: "alice".into(),
                    app_password: Password::new("app-pass-AbCdE"),
                    folder: String::new(),
                },
            },
            Request::AddImmich {
                req: ImmichRequest {
                    name: "Fotos".into(),
                    destination: Destination {
                        transport: "rclone".into(),
                        backend: Backend::Local,
                        root: "/srv/photos".into(),
                    },
                    password: Some(Password::new("s3cret-sftp-pass")),
                    server: "photos.example.com".into(),
                    api_key: Password::new("app-pass-AbCdE"),
                    library_id: None,
                    import_path: None,
                },
            },
        ];
        for request in requests {
            let envelope = Envelope { id: 7, request };
            let shown = format!("{envelope:?} {envelope:#?}");
            for secret in ["s3cret-sftp-pass", "app-pass-AbCdE"] {
                assert!(!shown.contains(secret), "{shown}");
            }
            // On the wire it is still the plain string the service needs.
            let wire = serde_json::to_string(&envelope).unwrap();
            assert!(
                wire.contains("s3cret-sftp-pass") || wire.contains("app-pass-AbCdE"),
                "{wire}"
            );
            let back: Envelope = serde_json::from_str(&wire).unwrap();
            assert_eq!(back.request, envelope.request);
        }
    }
}
