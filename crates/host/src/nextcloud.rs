//! The Nextcloud direct-mode profile wizard: detect over SSH, then save the profile.

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use filebridge_core::{Engine, Profile, TargetSpec};
use filebridge_integration_api::CheckStatus;
use filebridge_integration_nextcloud::{
    DetectInput, Detector, INTEGRATION_ID, LxcDetector, NextcloudSettings, ShellConnector,
    SshConnector, SshTarget,
};
use filebridge_ipc::{
    DetectNextcloudRequest, DetectNextcloudResponse, DetectStepView, ErrorBody, Extensions,
};
use filebridge_transport_api::{Backend, Destination};

fn err(code: &str, message: impl Into<String>) -> ErrorBody {
    ErrorBody {
        code: code.into(),
        message: message.into(),
    }
}

/// Runs the LXC detector and, when every step passes, saves the profile. `modes` overrides
/// the default `750`/`640` (the CLI exposes it; the app keeps the defaults).
pub async fn detect_nextcloud(
    engine: &Arc<Engine>,
    req: DetectNextcloudRequest,
    modes: Option<(String, String)>,
) -> Result<DetectNextcloudResponse, ErrorBody> {
    let ssh = SshTarget {
        host: req.host.clone(),
        port: req.port,
        user: req.user.clone(),
        key_file: req.key_file.clone().map(PathBuf::from),
        known_hosts_file: req.known_hosts_file.clone().map(PathBuf::from),
    };
    let shell = SshConnector::new()
        .connect(&ssh)
        .await
        .map_err(|e| err("ssh", e.message))?;
    let input = DetectInput {
        ssh: ssh.clone(),
        nextcloud_user: req.nextcloud_user.clone(),
        ctid: req.ctid,
        install_dir: req.install_dir.clone(),
    };
    let detection = LxcDetector.detect(shell.as_ref(), &input).await;
    let steps = detection
        .steps
        .iter()
        .map(|s| DetectStepView {
            id: s.check.id.clone(),
            status: match s.check.status {
                CheckStatus::Ok => "ok",
                CheckStatus::Warn => "warn",
                CheckStatus::Fail => "fail",
            }
            .into(),
            message: s.check.message.clone(),
            commands: s
                .commands
                .iter()
                .map(|c| format!("{}  (exit {})", c.command, c.status))
                .collect(),
        })
        .collect();
    let containers = detection
        .containers
        .iter()
        .map(|c| format!("{} {} ({})", c.ctid, c.name, c.status))
        .collect();

    let Some(mut nc) = detection.profile else {
        return Ok(DetectNextcloudResponse {
            steps,
            containers,
            profile: None,
        });
    };
    if let Some((dir_mode, file_mode)) = modes {
        nc.dir_mode = dir_mode;
        nc.file_mode = file_mode;
    }
    nc.validate().map_err(|e| err("invalid", e.message))?;
    let folder = req.folder.trim_matches('/');
    let root = if folder.is_empty() {
        nc.host_files_dir()
    } else {
        format!("{}/{folder}", nc.host_files_dir())
    };
    let settings = serde_json::to_value(NextcloudSettings::Direct(nc))
        .map_err(|e| err("internal", e.to_string()))?;
    let profile = engine
        .save_profile(Profile {
            id: String::new(),
            name: req.name,
            destination: Destination {
                transport: "rclone".into(),
                backend: Backend::Sftp {
                    host: ssh.host,
                    port: Some(ssh.port),
                    user: Some(ssh.user),
                    key_file: ssh.key_file,
                    known_hosts_file: ssh.known_hosts_file,
                    password: None,
                },
                root,
            },
            integration: Some(INTEGRATION_ID.into()),
            integration_settings: settings,
            target: TargetSpec::default(),
        })
        .await
        .map_err(|e| err("invalid", e.to_string()))?;
    Ok(DetectNextcloudResponse {
        steps,
        containers,
        profile: Some(profile),
    })
}

/// [`Extensions`] for `filebridge-service`: the requests that need the Nextcloud plugin.
pub struct NextcloudExtensions;

#[async_trait]
impl Extensions for NextcloudExtensions {
    async fn detect_nextcloud(
        &self,
        engine: &Arc<Engine>,
        req: DetectNextcloudRequest,
    ) -> Result<DetectNextcloudResponse, ErrorBody> {
        detect_nextcloud(engine, req, None).await
    }
}
