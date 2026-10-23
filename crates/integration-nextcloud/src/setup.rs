//! The profile wizard: from what the user types (SSH host, Nextcloud user, folder) to a
//! destination and settings ready to save, and the check every saved profile must pass.

use filebridge_integration_api::{CheckStatus, Destination, IntegrationError, Result};
use filebridge_transport_api::Backend;
use serde::{Deserialize, Serialize};

use crate::Nextcloud;
use crate::detect::{DetectInput, Detection};
use crate::direct::files_prefix;
use crate::profile::{NextcloudDirectProfile, NextcloudSettings};
use crate::shell::{RemoteShell, SshTarget};

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

/// A Nextcloud mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    Direct,
    Webdav,
}

impl Mode {
    /// The wire name, as serde writes it.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Direct => "direct",
            Self::Webdav => "webdav",
        }
    }
}

/// Why [`Nextcloud::advise`] recommends its mode. The wire form (`snake_case`) is also the
/// window's i18n key (`nextcloud.advice.<reason>`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AdviceReason {
    /// Every step passed: direct mode is ready.
    Ready,
    /// SSH did not connect (host down, refused, unknown or changed host key, sign-in refused).
    Unreachable,
    /// SSH connects, but the user lacks the rights direct mode needs: root (or `sudo -n`) for
    /// `pct` or the owner, or writing the user's files folder.
    Rights,
    /// No Nextcloud (or none usable) found there: no container, `config.php`, data folder,
    /// `occ`, or `occ` doesn't answer.
    NotFound,
}

impl AdviceReason {
    /// The wire name, as serde writes it.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ready => "ready",
            Self::Unreachable => "unreachable",
            Self::Rights => "rights",
            Self::NotFound => "not_found",
        }
    }
}

/// The wizard's recommendation.
#[derive(Debug, Clone)]
pub struct Advice {
    pub mode: Mode,
    pub reason: AdviceReason,
    /// The message of the step that decided it (or the connection error), for display.
    pub message: String,
    /// What detection found; `None` when SSH did not connect.
    pub detection: Option<Detection>,
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
        let detection = Self::detect(shell.as_ref(), req).await;
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
                key: None,
            },
            root,
        };
        Self::check_direct(&destination, &nc).map_err(|e| SetupError::Invalid(e.message))?;
        let settings = serde_json::to_value(NextcloudSettings::Direct(Box::new(nc)))
            .map_err(|e| SetupError::Invalid(e.to_string()))?;
        Ok(Setup {
            detection,
            profile: Some(ProfileParts {
                destination,
                settings,
            }),
        })
    }

    /// Runs each detector until one applies. The steps of those that didn't apply (no `pct`
    /// there) stay in front, so the Details panel shows why.
    async fn detect(shell: &dyn RemoteShell, req: &SetupRequest) -> Detection {
        let input = DetectInput {
            ssh: req.ssh.clone(),
            nextcloud_user: req.nextcloud_user.clone(),
            ctid: req.ctid,
            install_dir: req.install_dir.clone(),
        };
        let mut steps = Vec::new();
        let mut last = None;
        for detector in Self::detectors() {
            let mut found = detector.detect(shell, &input).await;
            steps.append(&mut found.steps);
            let applies = found.applies;
            last = Some(found);
            if applies {
                break;
            }
        }
        let mut detection = last.expect("there is at least one detector");
        detection.steps = steps;
        detection
    }

    /// Which mode suits this server, for the wizard's recommendation (ADR 0024): direct mode
    /// when SSH reaches the machine Nextcloud runs on with the rights it needs (every detection
    /// step passes), WebDAV otherwise, with the reason. Nothing is saved or changed on the server.
    pub async fn advise(&self, req: &SetupRequest) -> Advice {
        let shell = match self.connector.connect(&req.ssh).await {
            Ok(s) => s,
            Err(e) => {
                return Advice {
                    mode: Mode::Webdav,
                    reason: AdviceReason::Unreachable,
                    message: e.message,
                    detection: None,
                };
            }
        };
        let detection = Self::detect(shell.as_ref(), req).await;
        let failed = detection
            .steps
            .iter()
            .find(|s| s.check.status == CheckStatus::Fail);
        let (mode, reason, message) = match (failed, &detection.profile) {
            (None, Some(_)) => (
                Mode::Direct,
                AdviceReason::Ready,
                format!("Nextcloud found ({})", detection.detector),
            ),
            (Some(step), _) => (
                Mode::Webdav,
                if detection.lacks_rights {
                    AdviceReason::Rights
                } else {
                    AdviceReason::NotFound
                },
                step.check.message.clone(),
            ),
            (None, None) => (
                Mode::Webdav,
                AdviceReason::NotFound,
                "Nextcloud was not found on this server".to_string(),
            ),
        };
        Advice {
            mode,
            reason,
            message,
            detection: Some(detection),
        }
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
