//! The profile wizard: from what the user types (SSH host, Nextcloud user, folder) to a
//! destination and settings ready to save, and the check every saved profile must pass.

use filebridge_integration_api::{Destination, IntegrationError, Result};
use filebridge_transport_api::Backend;

use crate::Nextcloud;
use crate::detect::{DetectInput, Detection};
use crate::direct::files_prefix;
use crate::profile::{NextcloudDirectProfile, NextcloudSettings};
use crate::shell::SshTarget;

/// What the profile wizard asks the user for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SetupRequest {
    pub ssh: SshTarget,
    /// Nextcloud user id whose `files/` receives sends.
    pub nextcloud_user: String,
    /// Folder inside the user's files that receives sends (`""` for the root).
    pub folder: String,
    /// Container to use; when absent, the one running container named like Nextcloud (or, on a
    /// host without `pct`, Nextcloud on the host itself).
    pub ctid: Option<u32>,
    /// Nextcloud install dir where it runs; when absent, the usual places are tried.
    pub install_dir: Option<String>,
    /// Octal modes applied after a send; default `750` / `640`.
    pub modes: Option<(String, String)>,
}

/// A destination and this integration's settings for it, ready to save as a profile.
#[derive(Debug, Clone, PartialEq)]
pub struct ProfileParts {
    pub destination: Destination,
    pub settings: serde_json::Value,
}

/// What the wizard found, and the profile parts when every step passed.
#[derive(Debug, Clone)]
pub struct Setup {
    pub detection: Detection,
    pub profile: Option<ProfileParts>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SetupError {
    /// The SSH connection failed.
    Connect(String),
    /// Detection passed but the result can't be used (bad modes, say).
    Invalid(String),
}

impl Nextcloud {
    /// Connects over this integration's own connector, runs each detector until one applies,
    /// and builds the destination: SFTP to the same host as SSH, rooted at the user's files
    /// folder (plus `folder`), sent with the transport `transport`. The result passes
    /// [`Nextcloud::check_direct`], as every saved profile must.
    pub async fn set_up(
        &self,
        req: &SetupRequest,
        transport: &str,
    ) -> std::result::Result<Setup, SetupError> {
        let shell = self
            .connector
            .connect(&req.ssh)
            .await
            .map_err(|e| SetupError::Connect(e.message))?;
        let input = DetectInput {
            ssh: req.ssh.clone(),
            nextcloud_user: req.nextcloud_user.clone(),
            ctid: req.ctid,
            install_dir: req.install_dir.clone(),
        };
        // The first detector whose setup is on the host decides; the steps of those that
        // didn't apply (no `pct` there) stay in front, so the Details panel shows why.
        let mut steps = Vec::new();
        let mut last = None;
        for detector in Self::detectors() {
            let mut found = detector.detect(shell.as_ref(), &input).await;
            steps.append(&mut found.steps);
            let applies = found.applies;
            last = Some(found);
            if applies {
                break;
            }
        }
        let mut detection = last.expect("there is at least one detector");
        detection.steps = steps;
        let Some(mut nc) = detection.profile.clone() else {
            return Ok(Setup {
                detection,
                profile: None,
            });
        };
        if let Some((dir_mode, file_mode)) = &req.modes {
            nc.dir_mode = dir_mode.clone();
            nc.file_mode = file_mode.clone();
        }
        let folder = req.folder.trim_matches('/');
        let root = if folder.is_empty() {
            nc.host_files_dir()
        } else {
            format!("{}/{folder}", nc.host_files_dir())
        };
        let destination = Destination {
            transport: transport.to_owned(),
            backend: Backend::Sftp {
                host: nc.ssh.host.clone(),
                port: Some(nc.ssh.port),
                user: Some(nc.ssh.user.clone()),
                key_file: nc.ssh.key_file.clone(),
                known_hosts_file: nc.ssh.known_hosts_file.clone(),
                password: None,
            },
            root,
        };
        Self::check_direct(&destination, &nc).map_err(|e| SetupError::Invalid(e.message))?;
        let settings = serde_json::to_value(NextcloudSettings::Direct(nc))
            .map_err(|e| SetupError::Invalid(e.to_string()))?;
        Ok(Setup {
            detection,
            profile: Some(ProfileParts {
                destination,
                settings,
            }),
        })
    }

    /// What a direct-mode profile must hold: valid settings, files sent over SFTP to the host
    /// the integration reaches over SSH, and a root inside the user's files folder.
    pub fn check_direct(destination: &Destination, nc: &NextcloudDirectProfile) -> Result<()> {
        nc.validate()?;
        let Backend::Sftp { host, port, .. } = &destination.backend else {
            return Err(IntegrationError::fatal(
                "direct mode sends over SFTP to the Nextcloud host",
            ));
        };
        if *host != nc.ssh.host || port.unwrap_or(22) != nc.ssh.port {
            return Err(IntegrationError::fatal(format!(
                "files go to {host}:{}, but Nextcloud is reached at {}:{}; they must be the \
                 same host",
                port.unwrap_or(22),
                nc.ssh.host,
                nc.ssh.port
            )));
        }
        files_prefix(nc, &destination.root).map(drop)
    }
}
