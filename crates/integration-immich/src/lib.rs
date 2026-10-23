//! Immich integration (ADR 0020, `docs/adr/0020-immich-external-library.md`): files go
//! by the profile's transport (SFTP, or a local folder) into a folder of an Immich **external
//! library**, and after the send this asks Immich, through its API, to scan that library.
//! Nothing is uploaded through Immich, and Immich never moves or deletes the files.
//!
//! The API key lives in the OS keyring (ADR 0004); the host injects a `SecretResolver` to read
//! it, and the profile only holds its reference.

pub mod api;
#[cfg(feature = "fake")]
pub mod fake;
pub mod inspect;
pub mod settings;
pub mod skip;

use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use filebridge_integration_api::{
    AfterTransferReport, Check, CheckStatus, Destination, Integration, IntegrationError,
    IntegrationEvent, IntegrationSink, JobContext, NameRule, PreflightReport, Problem, RemotePath,
    Result, StepProgress, TransportCaps,
};
use filebridge_transport_api::{KeyringRef, SecretResolver};
use serde::{Deserialize, Serialize};

pub use api::{Api, ApiError, ApiErrorKind, Library, MediaTypes, PathValidation, QueueState};
pub use inspect::{Inspection, inspect};
pub use settings::{ImmichSettings, clean_path, covers, server_url, valid_library_id};
pub use skip::Skips;

pub const INTEGRATION_ID: &str = "immich";

/// The step key of the scan request, for [`IntegrationEvent::Stage`] (the UI's
/// `integration.immich.scan`).
pub const SCAN_STEP: &str = "immich.scan";
/// The step key while Immich scans (`integration.immich.wait`).
pub const WAIT_STEP: &str = "immich.wait";
/// [`IntegrationEvent::Waiting`] while the library queue is paused in Immich.
pub const QUEUE_PAUSED: &str = "immich.queue-paused";
/// Problem codes of the after-transfer step (also i18n keys under `integration.`).
pub const SCAN_UNFINISHED: &str = "immich.scan-unfinished";
pub const SCAN_NOT_FOLLOWED: &str = "immich.scan-not-followed";
/// The id of the wizard's check that Immich sees the destination folder ([`Immich::same_folder`]).
pub const SAME_FOLDER: &str = "same-folder";

/// How long the after-transfer step follows the scan, and how often it asks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScanWait {
    /// The first wait between two looks at the queue; doubled after each, up to `max_poll`.
    pub first_poll: Duration,
    pub max_poll: Duration,
    /// Past this, the step ends with a [`SCAN_UNFINISHED`] problem: Immich goes on scanning
    /// on its own, and the photos appear when it is done.
    pub timeout: Duration,
}

impl Default for ScanWait {
    fn default() -> Self {
        Self {
            first_poll: Duration::from_secs(2),
            max_poll: Duration::from_secs(30),
            timeout: Duration::from_secs(60 * 60),
        }
    }
}

/// What the after-transfer step saves once Immich accepted the scan, so a retry or a resumed
/// job waits for that scan instead of asking for another.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
struct ScanCheckpoint {
    scan_requested: bool,
    /// The library's asset count before the scan, to show how many it imported.
    #[serde(default)]
    assets_before: Option<u64>,
}

/// The [`Integration`]. Registered once; each job names its server, key and library.
pub struct Immich {
    api: Api,
    secrets: Arc<dyn SecretResolver>,
    wait: ScanWait,
}

/// What the profile wizard is given. The key is already in the keyring under `api_key_ref`; the
/// wizard reads it from `api_key` and never stores it.
#[derive(Clone)]
pub struct SetupRequest<'a> {
    /// What the user typed: `photos.example.com`, or an address with `/api`.
    pub server: &'a str,
    pub api_key: &'a str,
    pub api_key_ref: KeyringRef,
    /// `None`: the one library that imports `import_path`.
    pub library_id: Option<&'a str>,
    /// The destination's folder as Immich sees it; `None` means the same path as
    /// `destination_root` (Immich on the same machine, or its container mounts it there).
    pub import_path: Option<&'a str>,
    /// The destination's root folder (on the SFTP server, or on this machine).
    pub destination_root: &'a str,
}

/// The wizard's answer: each check, the libraries to choose from when it couldn't pick one, and
/// the settings to save when every check passed.
#[derive(Debug, Clone, Default)]
pub struct Setup {
    pub checks: Vec<Check>,
    pub candidates: Vec<Library>,
    pub settings: Option<ImmichSettings>,
}

impl Immich {
    pub fn new(secrets: Arc<dyn SecretResolver>) -> Self {
        Self {
            api: Api::new(),
            secrets,
            wait: ScanWait::default(),
        }
    }

    /// Follows scans with `wait` instead of [`ScanWait::default`] (tests).
    pub fn with_scan_wait(mut self, wait: ScanWait) -> Self {
        self.wait = wait;
        self
    }

    pub fn api(&self) -> &Api {
        &self.api
    }

    fn settings(ctx: &JobContext) -> Result<ImmichSettings> {
        let s = ctx.profile.settings_as::<ImmichSettings>()?;
        s.validate()?;
        Ok(s)
    }

    async fn key(&self, s: &ImmichSettings) -> Result<filebridge_transport_api::SecretValue> {
        self.secrets.resolve(&s.api_key).await.map_err(|e| {
            let retryable = e.is_retryable();
            let message = format!("the Immich API key: {}", e.message);
            IntegrationError { message, retryable }
        })
    }

    /// The profile wizard: normalise the address, check everything [`inspect`] checks, and
    /// build the settings when it all passes. Errors are what the user typed (`Invalid`).
    pub async fn set_up(&self, req: &SetupRequest<'_>) -> std::result::Result<Setup, String> {
        let server = server_url(req.server)?;
        let library_id = match req.library_id.map(str::trim).filter(|s| !s.is_empty()) {
            Some(id) if !valid_library_id(id) => {
                return Err(format!(
                    "{id:?} is not an Immich library id (it looks like \
                     6f1c2a4e-0b7d-4c55-9a8e-3d2f1b0c9e77; leave it empty to find it)"
                ));
            }
            id => id.map(str::to_ascii_lowercase),
        };
        let import_path = match req.import_path.map(str::trim).filter(|s| !s.is_empty()) {
            Some(p) => clean_path(p)?,
            None => clean_path(req.destination_root).map_err(|_| {
                "type the folder as Immich sees it (e.g. /mnt/photos): the destination folder \
                 is not an absolute path Immich could use"
                    .to_owned()
            })?,
        };
        if req.api_key.trim().is_empty() {
            return Err("enter the Immich API key".into());
        }
        let found = inspect(
            &self.api,
            &server,
            req.api_key.trim(),
            library_id.as_deref(),
            &import_path,
        )
        .await;
        let settings = if found.passed() {
            found.library.as_ref().map(|l| ImmichSettings {
                server: server.clone(),
                api_key: req.api_key_ref.clone(),
                library_id: l.id.to_ascii_lowercase(),
                import_path: import_path.clone(),
            })
        } else {
            None
        };
        Ok(Setup {
            checks: found.checks,
            candidates: found.candidates,
            settings,
        })
    }
}

impl Immich {
    /// Whether Immich sees the destination folder as `settings.import_path`: the host has
    /// written the file `probe` (a [`filebridge_transport_api::is_probe`] name) into the
    /// destination's root, `destination_root`, and this asks Immich (the library's path
    /// validation, which reads its own file system) whether `<import_path>/<probe>` is there.
    /// A hidden name, so no scan would ever import it. `Fail` when Immich doesn't find it.
    pub async fn same_folder(
        &self,
        settings: &ImmichSettings,
        key: &str,
        destination_root: &str,
        probe: &RemotePath,
    ) -> Check {
        let check = |status, message: String| Check {
            id: SAME_FOLDER.into(),
            status,
            message,
        };
        let import_path = settings.import_path.trim_end_matches('/');
        let seen_as = format!("{import_path}/{}", probe.as_str());
        let answer = self
            .api
            .validate(
                &settings.server,
                key,
                &settings.library_id,
                std::slice::from_ref(&seen_as),
            )
            .await;
        let v = match answer {
            Ok(v) => v,
            Err(e) => {
                return check(
                    CheckStatus::Warn,
                    format!(
                        "could not ask Immich whether it sees {destination_root} as \
                         {import_path}: {}",
                        e.message
                    ),
                );
            }
        };
        let Some(p) = v.into_iter().find(|p| p.import_path == seen_as) else {
            return check(
                CheckStatus::Warn,
                format!("Immich said nothing about the test file {seen_as}"),
            );
        };
        let message = p.message.unwrap_or_default();
        // Immich validates folders: a file it finds is "Not a directory". Found either way.
        if p.is_valid || message.eq_ignore_ascii_case("not a directory") {
            check(
                CheckStatus::Ok,
                format!("Immich sees {destination_root} as {import_path}"),
            )
        } else if message.contains("ENOENT") || message.contains("does not exist") {
            check(
                CheckStatus::Fail,
                format!(
                    "Immich doesn't see the files sent to {destination_root}: a test file \
                     written there is not in {import_path} as Immich sees it. Type the folder \
                     as Immich sees it (e.g. its path inside Immich's container), or send to \
                     the folder Immich imports"
                ),
            )
        } else {
            check(
                CheckStatus::Warn,
                format!("could not tell whether Immich sees {destination_root}: {message}"),
            )
        }
    }

    /// The rules of the library for this profile's folder; the server's media types when it
    /// says them, Immich's own lists otherwise.
    async fn skips(&self, s: &ImmichSettings, key: &str) -> std::result::Result<Skips, ApiError> {
        let library = self.api.library(&s.server, key, &s.library_id).await?;
        let media = self.api.media_types(&s.server).await.ok();
        Ok(Skips::new(&library, &s.import_path, media.as_ref()))
    }

    async fn asset_count(&self, s: &ImmichSettings, key: &str) -> Option<u64> {
        self.api
            .library(&s.server, key, &s.library_id)
            .await
            .ok()
            .map(|l| l.asset_count)
    }

    /// Follows the library queue until it is idle, with backoff, up to the timeout. A problem
    /// when it couldn't: the scan was accepted either way.
    async fn wait_for_scan(
        &self,
        s: &ImmichSettings,
        key: &str,
        assets_before: Option<u64>,
        progress: &IntegrationSink,
    ) -> Option<Problem> {
        progress.send(IntegrationEvent::Stage(WAIT_STEP.into()));
        let started = Instant::now();
        let mut pause = self.wait.first_poll;
        let mut last_error: Option<String>;
        let problem = |code: &str, message: String| Problem {
            code: code.into(),
            message,
            path: None,
            retryable: false,
            fix: None,
        };
        loop {
            match self.api.library_queue(&s.server, key).await {
                Ok(q) if q.idle() => return None,
                Ok(q) => {
                    last_error = None;
                    if q.paused {
                        progress.send(IntegrationEvent::Waiting(QUEUE_PAUSED.into()));
                    }
                    let imported = match (assets_before, self.asset_count(s, key).await) {
                        (Some(before), Some(now)) => now.saturating_sub(before),
                        _ => 0,
                    };
                    progress.send(IntegrationEvent::Progress(StepProgress {
                        step: WAIT_STEP.into(),
                        done: 0,
                        total: 0,
                        items: imported,
                        current: None,
                    }));
                }
                Err(e) if e.is_retryable() => last_error = Some(e.message),
                Err(e) => {
                    return Some(problem(
                        SCAN_NOT_FOLLOWED,
                        format!(
                            "Immich is scanning the library, but filebridge can't follow it: \
                             the API key needs the queue.read permission (job.read before \
                             Immich 2) ({})",
                            e.message
                        ),
                    ));
                }
            }
            if started.elapsed() >= self.wait.timeout {
                let mins = self.wait.timeout.as_secs().div_ceil(60);
                let why = last_error
                    .map(|e| format!(" (the last answer: {e})"))
                    .unwrap_or_default();
                return Some(problem(
                    SCAN_UNFINISHED,
                    format!(
                        "Immich was still scanning the library after {mins} min{why}; the \
                         new photos appear when it is done"
                    ),
                ));
            }
            tokio::time::sleep(pause.min(self.wait.timeout.saturating_sub(started.elapsed())))
                .await;
            pause = (pause * 2).min(self.wait.max_poll);
        }
    }
}

#[async_trait]
impl Integration for Immich {
    fn id(&self) -> &str {
        INTEGRATION_ID
    }

    /// Immich only reads the folder; it needs nothing from the transport.
    fn required_caps(&self) -> TransportCaps {
        TransportCaps::empty()
    }

    /// Immich imports any name its file system holds; the general rules are enough.
    fn name_rules(&self, _settings: &serde_json::Value) -> Vec<Box<dyn NameRule>> {
        Vec::new()
    }

    fn check_profile(&self, destination: &Destination, settings: &serde_json::Value) -> Result<()> {
        let s: ImmichSettings = serde_json::from_value(settings.clone())
            .map_err(|e| IntegrationError::fatal(format!("Immich settings: {e}")))?;
        s.check(destination)
    }

    async fn preflight(&self, ctx: &JobContext) -> Result<PreflightReport> {
        let s = Self::settings(ctx)?;
        let key = match self.key(&s).await {
            Ok(k) => k,
            Err(e) => {
                return Ok(PreflightReport {
                    writable: true,
                    checks: vec![Check {
                        id: "api-key".into(),
                        status: CheckStatus::Fail,
                        message: e.message,
                    }],
                    ..PreflightReport::default()
                });
            }
        };
        let found = inspect(
            &self.api,
            &s.server,
            key.expose(),
            Some(&s.library_id),
            &s.import_path,
        )
        .await;
        Ok(PreflightReport {
            // The transport's preflight says whether the folder is writable; Immich only reads.
            writable: true,
            latency_ms: found.latency_ms,
            checks: found.checks,
            ..PreflightReport::default()
        })
    }

    /// Files the library's scan will pass over: excluded by its patterns, hidden, or of a type
    /// Immich doesn't import.
    async fn will_skip(&self, ctx: &JobContext, files: &[RemotePath]) -> Result<Vec<Problem>> {
        let s = Self::settings(ctx)?;
        let key = self.key(&s).await?;
        let skips = self.skips(&s, key.expose()).await?;
        Ok(skips.check_all(files))
    }

    /// Queues a scan of the whole library (Immich's API has no scan by path; its scan only
    /// imports what is new or changed, so the files already there cost a look each), then
    /// follows Immich's library queue until the scan is done. Once Immich accepted the scan
    /// the step saves a checkpoint, so running it again only waits. The sent files the scan
    /// will pass over are problems of the report.
    async fn after_transfer(
        &self,
        ctx: &JobContext,
        changed: &[RemotePath],
        progress: IntegrationSink,
    ) -> Result<AfterTransferReport> {
        let s = Self::settings(ctx)?;
        let earlier: ScanCheckpoint = ctx
            .checkpoint
            .clone()
            .and_then(|v| serde_json::from_value(v).ok())
            .unwrap_or_default();
        let key = self.key(&s).await?;
        let mut problems = match self.skips(&s, key.expose()).await {
            Ok(skips) => skips.check_all(changed),
            Err(e) if e.kind == ApiErrorKind::NotFound => {
                return Err(IntegrationError::fatal(format!(
                    "Immich has no library {} any more ({})",
                    s.library_id, e.message
                )));
            }
            // The scan below says what is wrong with the key or the server.
            Err(_) => Vec::new(),
        };
        let skipped = problems.len() as u64;
        let assets_before = if earlier.scan_requested {
            earlier.assets_before
        } else {
            progress.send(IntegrationEvent::Stage(SCAN_STEP.into()));
            let before = self.asset_count(&s, key.expose()).await;
            self.request_scan(&s, key.expose()).await?;
            progress
                .checkpoint(
                    serde_json::to_value(ScanCheckpoint {
                        scan_requested: true,
                        assets_before: before,
                    })
                    .unwrap_or_default(),
                )
                .await;
            before
        };
        problems.extend(
            self.wait_for_scan(&s, key.expose(), assets_before, &progress)
                .await,
        );
        for p in &problems {
            progress.send(IntegrationEvent::Problem(p.clone()));
        }
        Ok(AfterTransferReport {
            updated: vec![s.import_path.clone()],
            items: (changed.len() as u64).saturating_sub(skipped),
            problems,
        })
    }
}

impl Immich {
    async fn request_scan(&self, s: &ImmichSettings, key: &str) -> Result<()> {
        self.api
            .scan(&s.server, key, &s.library_id)
            .await
            .map_err(|e| match e.kind {
                ApiErrorKind::Auth => IntegrationError::fatal(format!(
                    "Immich refused the API key, so the library was not scanned: set the key \
                     again ({})",
                    e.message
                )),
                ApiErrorKind::Forbidden => IntegrationError::fatal(format!(
                    "the API key may not scan the library: it needs the library.update \
                     permission, and its user must be an administrator ({})",
                    e.message
                )),
                ApiErrorKind::NotFound => IntegrationError::fatal(format!(
                    "Immich has no library {} any more ({})",
                    s.library_id, e.message
                )),
                _ => e.into(),
            })
    }
}
