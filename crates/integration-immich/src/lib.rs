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

use std::sync::Arc;

use async_trait::async_trait;
use filebridge_integration_api::{
    AfterTransferReport, Check, CheckStatus, Destination, Integration, IntegrationError,
    IntegrationEvent, IntegrationSink, JobContext, NameRule, PreflightReport, RemotePath, Result,
    TransportCaps,
};
use filebridge_transport_api::{KeyringRef, SecretResolver};

pub use api::{Api, ApiError, ApiErrorKind, Library, PathValidation};
pub use inspect::{Inspection, inspect};
pub use settings::{ImmichSettings, clean_path, covers, server_url, valid_library_id};

pub const INTEGRATION_ID: &str = "immich";

/// The step key of the scan, for [`IntegrationEvent::Stage`] (the UI's `integration.immich.scan`).
pub const SCAN_STEP: &str = "immich.scan";

/// The [`Integration`]. Registered once; each job names its server, key and library.
pub struct Immich {
    api: Api,
    secrets: Arc<dyn SecretResolver>,
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
        }
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

    /// Queues a scan of the whole library: Immich's API has no scan by path. Immich's scan
    /// only imports what is new or changed, so the files already there cost a look each.
    async fn after_transfer(
        &self,
        ctx: &JobContext,
        changed: &[RemotePath],
        progress: IntegrationSink,
    ) -> Result<AfterTransferReport> {
        let s = Self::settings(ctx)?;
        progress.send(IntegrationEvent::Stage(SCAN_STEP.into()));
        let key = self.key(&s).await?;
        self.api
            .scan(&s.server, key.expose(), &s.library_id)
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
            })?;
        Ok(AfterTransferReport {
            updated: vec![s.import_path.clone()],
            items: changed.len() as u64,
            problems: Vec::new(),
        })
    }
}
