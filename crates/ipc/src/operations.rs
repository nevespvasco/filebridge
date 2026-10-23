//! The catalogue of operations: every request a client can make, written once, with its
//! arguments and its answer's type. Everything that carries an operation is generated from it:
//! the wire [`Request`](crate::Request), the [`Operations`](crate::Operations) trait (implemented
//! by the [`Client`](crate::Client) over the socket and by the engine adapter the service serves),
//! the server's dispatch, and the desktop app's Tauri commands.
//!
//! A new operation is one line here plus its method in the engine adapter
//! (`crates/host/src/operations.rs`). Bump [`PROTOCOL_VERSION`](crate::PROTOCOL_VERSION) with it.

/// Calls `$m!` with the catalogue: one `Variant name(arg: Type, …) -> Answer;` per operation, with
/// its doc comment. Use it to generate code that must cover every operation.
#[macro_export]
macro_rules! for_each_operation {
    ($m:ident) => {
        $m! {
            /// The protocol and service versions; a mismatch means the window and the service
            /// don't fit.
            Ping ping() -> $crate::Pong;
            ListProfiles list_profiles() -> Vec<$crate::Profile>;
            /// Saves a profile, giving it an id if it has none. Refused if its plugins don't
            /// accept it.
            SaveProfile save_profile(profile: $crate::Profile) -> $crate::Profile;
            DeleteProfile delete_profile(id: String) -> ();
            /// Connection test: the transport's and the integration's preflight.
            TestProfile test_profile(id: String) -> $crate::ProfileTest;
            /// A destination's space from its last check; `refresh` checks it first (network).
            ProfileStorage profile_storage(id: String, refresh: bool) -> $crate::StorageView;
            /// Folders inside `dir` of a profile's destination (or one being edited), at most 2000;
            /// `prefix` narrows by the start of the name.
            Browse browse(target: $crate::BrowseTarget, dir: Option<String>, prefix: Option<String>) -> $crate::BrowseResult;
            /// Whether a folder name is acceptable at the destination, and the fix if not.
            CheckFolderName check_folder_name(target: $crate::BrowseTarget, name: String) -> $crate::FolderNameCheck;
            /// Nextcloud direct-mode profile wizard (detection over SSH: a Proxmox LXC, or
            /// Nextcloud on the SSH host itself). Saves the profile when every step passes.
            DetectNextcloud detect_nextcloud(req: $crate::DetectNextcloudRequest) -> $crate::DetectNextcloudResponse;
            /// Nextcloud WebDAV-mode profile wizard: checks the address, the sign-in, the folder
            /// and the space, and saves the profile (its app password in the keyring) when they
            /// pass. Same answer as `detect_nextcloud`.
            AddNextcloudWebdav add_nextcloud_webdav(req: $crate::NextcloudWebdavRequest) -> $crate::DetectNextcloudResponse;
            /// Immich profile wizard (ADR 0020): checks the destination, the Immich address, the
            /// API key, the external library and its folder, and saves the profile (the key in
            /// the keyring) when they pass. Same answer as `detect_nextcloud`; ambiguous
            /// libraries are listed in `containers`.
            AddImmich add_immich(req: $crate::ImmichRequest) -> $crate::DetectNextcloudResponse;
            /// The folder the last transfer to a profile was sent into (`None` = its root, none
            /// yet, or one its name rules no longer accept), which wizard step 2 preselects.
            LastTargetDir last_target_dir(profile_id: String) -> Option<String>;
            /// A draft job; nothing is read from the sources yet.
            CreateJob create_job(sources: Vec<String>, profile_id: String, options: $crate::SendOptions) -> String;
            /// Replaces a job's send options before it is sent (Draft, AwaitingApproval, Ready).
            SetOptions set_options(job_id: String, options: $crate::SendOptions) -> ();
            Analyse analyse(job_id: String) -> $crate::AnalysisSummary;
            /// Per-rule counts; with `query`, only issues whose source path contains it.
            RuleCounts rule_counts(job_id: String, query: Option<String>) -> Vec<$crate::RuleCount>;
            /// One page of a job's issues (optionally of one rule), by id after `after_id`.
            Issues issues(job_id: String, rule: Option<String>, after_id: Option<i64>, limit: u32, query: Option<String>) -> Vec<$crate::IssueView>;
            Decide decide(job_id: String, issue_id: i64, decision: $crate::DecisionInput) -> ();
            /// One decision for every pending issue of the job (or of `rule`). Not `edit`.
            /// Answers with the number of issues changed.
            DecideAll decide_all(job_id: String, rule: Option<String>, decision: $crate::DecisionInput, query: Option<String>) -> u64;
            /// One decision (not `edit`) for each listed issue; the ones the name rules refuse
            /// keep theirs and are reported.
            DecideMany decide_many(job_id: String, issue_ids: Vec<i64>, decision: $crate::DecisionInput) -> $crate::DecideManyResult;
            Start start(job_id: String) -> ();
            Pause pause(job_id: String) -> ();
            Resume resume(job_id: String) -> ();
            Cancel cancel(job_id: String) -> ();
            /// Leaves one file not sent yet out of a started send (Transferring, Retrying,
            /// Waiting, Paused): Skipped at once, never sent by a resume, stopping the batch
            /// that is sending it. Shown skipped in the Files tab and the report.
            SkipEntry skip_entry(job_id: String, idx: u64) -> ();
            /// A page of the jobs matching `filter`, with how many match in all.
            ListJobs list_jobs(filter: $crate::JobFilter, limit: u32, offset: u32) -> $crate::JobPage;
            /// Names a job (any state); a blank name clears it.
            SetLabel set_label(job_id: String, label: Option<String>) -> ();
            /// A new draft with a finished job's sources and options, to `profile_id` or the same
            /// profile; its analysis reuses the earlier name decisions.
            SendAgain send_again(job_id: String, profile_id: Option<String>) -> String;
            Job job(job_id: String) -> $crate::JobSummary;
            /// Whether a job's sources can be freed safely, and why not; no I/O at the source.
            ReclaimStatus reclaim_status(job_id: String) -> $crate::ReclaimStatus;
            /// Re-reads the job's sources (read-only) and compares them with the manifest.
            RecheckSources recheck_sources(job_id: String) -> $crate::ReclaimStatus;
            /// Hides a job from the reclaimable total (or shows it again).
            DismissReclaim dismiss_reclaim(job_id: String, dismissed: bool) -> ();
            /// Writes a finished job's report to `req.path` on the service's machine.
            ExportReport export_report(job_id: String, req: $crate::ReportRequest) -> $crate::ExportResult;
            /// Two finished jobs' reports compared, files matched by source path: the counts per
            /// kind of change, and the first `limit` changes (only of `kind` when given).
            DiffReports diff_reports(job_a: String, job_b: String, kind: Option<$crate::ReportChangeKind>, limit: u32) -> $crate::ReportDiff;
            /// Checks a sealed report file on the service's machine (the JSON or its HTML copy;
            /// the other one beside it too): whether anything changed since the export.
            CheckReport check_report(path: String) -> $crate::ReportCheck;
            /// Milestones of every job and profile changes, newest first, before `before_id`.
            Activity activity(before_id: Option<i64>, limit: u32) -> Vec<$crate::ActivityItem>;
            /// A job's files per Category, most bytes first.
            JobCategories job_categories(job_id: String) -> Vec<$crate::CategoryCount>;
            /// Folders and files sent before, newest first, for "Recently sent from".
            RecentSources recent_sources(limit: u32) -> Vec<$crate::RecentSource>;
            /// Gives an SSH profile filebridge's own key (ADR 0012): generated here, added with
            /// `restrict` to the server's authorized_keys, kept in the OS keyring, used from then
            /// on. Undone if any step fails.
            InstallAppKey install_app_key(profile_id: String) -> $crate::AccessReport;
            /// Deletes filebridge's key from the server and the keyring, puts the profile back on
            /// its key file or agent, and proves the old key no longer signs in.
            RemoveAccess remove_access(profile_id: String) -> $crate::AccessReport;
            /// Connects to an SSH server only far enough to read its host key (never signs in) and
            /// says whether `known_hosts_file` (default `~/.ssh/known_hosts`) knows it, has no key
            /// for the host, or has another one. Writes nothing.
            ScanHostKey scan_host_key(host: String, port: u16, known_hosts_file: Option<String>) -> $crate::HostKeyView;
            /// Adds the server's host key to the known_hosts file once the user has checked its
            /// `fingerprint`: only when the host has no key there yet and the server still
            /// presents that key. A changed key is refused (`host_key_changed`), never replaced.
            TrustHostKey trust_host_key(host: String, port: u16, known_hosts_file: Option<String>, fingerprint: String) -> $crate::HostKeyView;
            /// Stores the profile's password in the OS keyring (`None` removes it); the profile
            /// keeps only its reference (ADR 0004). Never logged, never in a Details panel.
            SetProfilePassword set_profile_password(profile_id: String, password: Option<$crate::Password>) -> $crate::Profile;
            /// A draft "Repair destination" job for a folder of a profile (`None` = its root):
            /// `analyse`, the decisions and `start` then work on it like on a send.
            CreateRepairJob create_repair_job(profile_id: String, dir: Option<String>) -> String;
            /// How many entries a Repair job would rename with its decisions as they are: what
            /// the confirmation counts. Refused while a decision is pending.
            PlannedRenames planned_renames(job_id: String) -> u64;
            /// What each chosen source is and what is wrong with it, without reading contents.
            InspectSources inspect_sources(paths: Vec<String>) -> Vec<$crate::SourceInfo>;
            /// The pinned sources, in the order they were pinned, each checked for whether it is
            /// there now.
            PinnedSources pinned_sources() -> Vec<$crate::PinnedSource>;
            /// Pins an absolute path (it needn't exist now), or unpins it. At most 100 pins.
            PinSource pin_source(path: String, pinned: bool) -> ();
            /// The disks mounted on the service's computer that files can be sent from (external
            /// and removable disks, memory cards, disks under `/media` or `/mnt`).
            Volumes volumes() -> Vec<$crate::VolumeView>;
            /// Measures a folder: files and bytes of each item directly inside it, most bytes
            /// first. `scan_id` (the caller's) names it in the `folder_sizes_progress` events
            /// sent while it runs and for `cancel_folder_sizes`; cancelled, it answers with what
            /// it counted (`complete: false`). At most 4 run at once.
            FolderSizes folder_sizes(scan_id: String, path: String) -> $crate::FolderSizes;
            /// Stops a `folder_sizes` measurement; nothing happens if it already ended.
            CancelFolderSizes cancel_folder_sizes(scan_id: String) -> ();
            /// One page of a job's files, filtered and sorted (at most 500).
            Entries entries(job_id: String, filter: $crate::EntryFilter, after: Option<$crate::EntryCursor>, limit: u32) -> $crate::EntryPage;
            /// Counts for the Files tab's filter chips, over the whole job.
            EntryFacets entry_facets(job_id: String) -> $crate::EntryFacets;
            /// Everything about one file of a job.
            Entry entry(job_id: String, idx: u64) -> $crate::EntryDetail;
            /// A small preview of a file's source image, made from the local source (read only)
            /// and cached; `None` when it has none (not an image, a video, a folder, a Repair job,
            /// or the source is gone).
            EntryThumbnail entry_thumbnail(job_id: String, idx: u64) -> Option<$crate::EntryThumbnail>;
            /// A job's log: oldest first after `after_id`, otherwise newest first before
            /// `before_id` (the newest page when both are absent).
            JobLog job_log(job_id: String, after_id: Option<i64>, before_id: Option<i64>, limit: u32) -> Vec<$crate::LogEntry>;
            /// The app-wide bandwidth limit and send window.
            GetSettings get_settings() -> $crate::AppSettings;
            /// Validates, saves and applies them at once, to running sends too.
            SaveSettings save_settings(settings: $crate::AppSettings) -> ();
            /// The engine's recent log lines (info and above, at most 2000, oldest first), with
            /// secrets scrubbed: Settings → About → "Copy log", for bug reports.
            ServiceLog service_log() -> Vec<String>;
        }
    };
}

/// Generates the wire request, the trait, the client's side and the server's dispatch.
macro_rules! define_operations {
    ($(
        $(#[$doc:meta])*
        $variant:ident $name:ident ( $($arg:ident : $ty:ty),* ) -> $ret:ty;
    )*) => {
        /// One request on the wire (`{"type": "<operation>", …arguments}`, under the call's `request`).
        #[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize, specta::Type)]
        #[serde(tag = "type", rename_all = "snake_case")]
        pub enum Request {
            $(
                $(#[$doc])*
                $variant { $($arg: $ty),* },
            )*
            /// From now on, this connection also receives every engine [`Event`](crate::Event).
            Subscribe,
        }

        /// Everything a client can ask of the service, one method per operation in the
        /// catalogue. [`Client`](crate::Client) implements it over the socket; the service
        /// serves an implementation backed by the engine.
        #[async_trait::async_trait]
        pub trait Operations: Send + Sync {
            $(
                $(#[$doc])*
                async fn $name(&self, $($arg: $ty),*) -> Result<$ret, crate::ErrorBody>;
            )*
        }

        #[async_trait::async_trait]
        impl Operations for crate::Client {
            $(
                async fn $name(&self, $($arg: $ty),*) -> Result<$ret, crate::ErrorBody> {
                    self.call::<$ret>(Request::$variant { $($arg),* }).await
                }
            )*
        }

        /// Runs one request and serializes its (typed) answer.
        pub(crate) async fn dispatch(
            ops: &dyn Operations,
            request: Request,
        ) -> Result<serde_json::Value, crate::ErrorBody> {
            match request {
                $(
                    Request::$variant { $($arg),* } => {
                        let answer: $ret = ops.$name($($arg),*).await?;
                        serde_json::to_value(answer).map_err(|e| crate::ErrorBody {
                            code: "internal".into(),
                            message: e.to_string(),
                        })
                    }
                )*
                Request::Subscribe => unreachable!("handled per connection"),
            }
        }
    };
}

for_each_operation!(define_operations);
