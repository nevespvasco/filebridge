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
            /// Nextcloud direct-mode profile wizard (Proxmox LXC detection over SSH). Saves the
            /// profile when every step passes.
            DetectNextcloud detect_nextcloud(req: $crate::DetectNextcloudRequest) -> $crate::DetectNextcloudResponse;
            /// A draft job; nothing is read from the sources yet.
            CreateJob create_job(sources: Vec<String>, profile_id: String, options: $crate::SendOptions) -> String;
            /// Replaces a job's send options before it is sent (Draft, AwaitingApproval, Ready).
            SetOptions set_options(job_id: String, options: $crate::SendOptions) -> ();
            Analyse analyse(job_id: String) -> $crate::AnalysisSummary;
            RuleCounts rule_counts(job_id: String) -> Vec<$crate::RuleCount>;
            /// One page of a job's issues (optionally of one rule), by id after `after_id`.
            Issues issues(job_id: String, rule: Option<String>, after_id: Option<i64>, limit: u32) -> Vec<$crate::IssueView>;
            Decide decide(job_id: String, issue_id: i64, decision: $crate::DecisionInput) -> ();
            /// One decision for every pending issue of the job (or of `rule`). Not `edit`.
            /// Answers with the number of issues changed.
            DecideAll decide_all(job_id: String, rule: Option<String>, decision: $crate::DecisionInput) -> u64;
            Start start(job_id: String) -> ();
            Pause pause(job_id: String) -> ();
            Resume resume(job_id: String) -> ();
            Cancel cancel(job_id: String) -> ();
            /// Newest first.
            ListJobs list_jobs(limit: u32, offset: u32) -> Vec<$crate::JobSummary>;
            Job job(job_id: String) -> $crate::JobSummary;
        }
    };
}

/// Generates the wire request, the trait, the client's side and the server's dispatch.
macro_rules! define_operations {
    ($(
        $(#[$doc:meta])*
        $variant:ident $name:ident ( $($arg:ident : $ty:ty),* ) -> $ret:ty;
    )*) => {
        /// One request on the wire (`{"id": n, "type": "<operation>", …arguments}`).
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
