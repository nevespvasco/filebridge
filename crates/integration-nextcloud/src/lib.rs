//! Nextcloud integration. Phase 1 ships direct mode (SFTP into the data folder, then owner/mode
//! and `occ files:scan --path` over SSH); WebDAV mode lands in Phase 2.
//!
//! See `docs/05-integration-nextcloud.md`.

pub mod dedicated;
pub mod detect;
pub mod direct;
pub mod fake;
pub mod known_hosts;
pub mod parse;
pub mod profile;
pub mod quote;
pub mod rules;
pub mod setup;
pub mod shell;
pub mod ssh;
pub mod webdav;

use std::sync::Arc;

use async_trait::async_trait;
use filebridge_integration_api::{
    AfterTransferReport, Destination, Integration, IntegrationError, IntegrationSink, JobContext,
    NameRule, PreflightReport, RemotePath, Result, TransportCaps,
};

pub use detect::{
    DetectInput, Detection, Detector, DockerDetector, HostDetector, LxcDetector, SnapDetector,
};
pub use direct::DirectOptions;
pub use profile::{Access, NextcloudDirectProfile, NextcloudSettings, Owner, RunAs, Runtime};
pub use rules::{NextcloudNfc, NextcloudReserved};
pub use setup::{Advice, AdviceReason, Mode, ProfileParts, Setup, SetupError, SetupRequest};
pub use shell::{Output, RemoteShell, ShellConnector, SshTarget};
pub use ssh::{SshConnector, SshShell};
pub use webdav::NextcloudWebdavProfile;

pub const INTEGRATION_ID: &str = "nextcloud";

/// The Nextcloud [`Integration`]. Registered once; each job connects with its profile's target.
pub struct Nextcloud {
    connector: Arc<dyn ShellConnector>,
    options: DirectOptions,
}

impl Nextcloud {
    pub fn new(connector: Arc<dyn ShellConnector>) -> Self {
        Self {
            connector,
            options: DirectOptions::default(),
        }
    }

    /// Over real SSH.
    pub fn with_ssh() -> Self {
        Self::new(Arc::new(SshConnector::new()))
    }

    pub fn with_options(mut self, options: DirectOptions) -> Self {
        self.options = options;
        self
    }

    /// Detectors the profile wizard tries, in order, until one applies: a Proxmox node (`pct`)
    /// first, then a Docker container (the official image or AIO), the snap, and else Nextcloud
    /// installed on the SSH host itself.
    pub fn detectors() -> Vec<Box<dyn Detector>> {
        vec![
            Box::new(LxcDetector),
            Box::new(DockerDetector),
            Box::new(SnapDetector),
            Box::new(HostDetector),
        ]
    }

    fn settings(ctx: &JobContext) -> Result<NextcloudSettings> {
        let settings = ctx.profile.settings_as::<NextcloudSettings>()?;
        match &settings {
            NextcloudSettings::Direct(p) => p.validate()?,
            NextcloudSettings::Webdav(p) => p.validate()?,
        }
        Ok(settings)
    }
}

#[async_trait]
impl Integration for Nextcloud {
    fn id(&self) -> &str {
        INTEGRATION_ID
    }

    /// Direct mode sets owner and mode itself over SSH, so it needs nothing from the transport.
    fn required_caps(&self) -> TransportCaps {
        TransportCaps::empty()
    }

    fn name_rules(&self, _settings: &serde_json::Value) -> Vec<Box<dyn NameRule>> {
        vec![Box::new(NextcloudNfc), Box::new(NextcloudReserved)]
    }

    fn check_profile(&self, destination: &Destination, settings: &serde_json::Value) -> Result<()> {
        match serde_json::from_value(settings.clone())
            .map_err(|e| IntegrationError::fatal(format!("Nextcloud settings: {e}")))?
        {
            NextcloudSettings::Direct(nc) => Self::check_direct(destination, &nc),
            NextcloudSettings::Webdav(nc) => nc.check(destination),
        }
    }

    async fn preflight(&self, ctx: &JobContext) -> Result<PreflightReport> {
        match Self::settings(ctx)? {
            NextcloudSettings::Direct(p) => {
                direct::preflight(&p, ctx, self.connector.as_ref()).await
            }
            NextcloudSettings::Webdav(p) => webdav::preflight(&p, ctx),
        }
    }

    async fn after_transfer(
        &self,
        ctx: &JobContext,
        changed: &[RemotePath],
        progress: IntegrationSink,
    ) -> Result<AfterTransferReport> {
        match Self::settings(ctx)? {
            NextcloudSettings::Direct(p) => {
                direct::after_transfer(
                    &p,
                    ctx,
                    changed,
                    &progress,
                    self.connector.as_ref(),
                    &self.options,
                )
                .await
            }
            NextcloudSettings::Webdav(_) => Ok(webdav::after_transfer(changed)),
        }
    }
}
