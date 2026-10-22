//! The Nextcloud profile wizard for the hosts: the plugin detects and builds the profile
//! (`Nextcloud::set_up`); this turns the ipc request into its input and saves the result.

use std::path::PathBuf;
use std::sync::Arc;

use filebridge_core::{Engine, PreflightView, Profile, TargetSpec};
use filebridge_integration_nextcloud::{
    INTEGRATION_ID, Nextcloud, SetupError, SetupRequest, SshTarget,
};
use filebridge_ipc::{DetectNextcloudRequest, DetectNextcloudResponse, DetectStepView, ErrorBody};

fn err(code: &str, message: impl Into<String>) -> ErrorBody {
    ErrorBody {
        code: code.into(),
        message: message.into(),
    }
}

/// Runs the wizard and, when every step passes, saves the profile with `transport`. `modes`
/// overrides the default `750`/`640` (the CLI exposes it; the app keeps the defaults).
async fn detect_nextcloud(
    engine: &Engine,
    nextcloud: &Nextcloud,
    transport: &str,
    req: DetectNextcloudRequest,
    modes: Option<(String, String)>,
) -> Result<DetectNextcloudResponse, ErrorBody> {
    let setup = nextcloud
        .set_up(
            &SetupRequest {
                ssh: SshTarget {
                    host: req.host,
                    port: req.port,
                    user: req.user,
                    key_file: req.key_file.map(PathBuf::from),
                    known_hosts_file: req.known_hosts_file.map(PathBuf::from),
                },
                nextcloud_user: req.nextcloud_user,
                folder: req.folder,
                ctid: req.ctid,
                install_dir: req.install_dir,
                modes,
            },
            transport,
        )
        .await
        .map_err(|e| match e {
            SetupError::Connect(m) => err("ssh", m),
            SetupError::Invalid(m) => err("invalid", m),
        })?;
    let steps = setup
        .detection
        .steps
        .iter()
        .map(|s| DetectStepView {
            id: s.check.id.clone(),
            status: PreflightView::status_text(s.check.status).into(),
            message: s.check.message.clone(),
            commands: s
                .commands
                .iter()
                .map(|c| format!("{}  (exit {})", c.command, c.status))
                .collect(),
        })
        .collect();
    let containers = setup
        .detection
        .containers
        .iter()
        .map(|c| format!("{} {} ({})", c.ctid, c.name, c.status))
        .collect();
    let profile = match setup.profile {
        None => None,
        Some(parts) => Some(
            engine
                .save_profile(Profile {
                    id: String::new(),
                    name: req.name,
                    destination: parts.destination,
                    integration: Some(INTEGRATION_ID.into()),
                    integration_settings: parts.settings,
                    target: TargetSpec::default(),
                })
                .await
                .map_err(|e| err("invalid", e.to_string()))?,
        ),
    };
    Ok(DetectNextcloudResponse {
        detector: setup.detection.detector.clone(),
        steps,
        containers,
        profile,
    })
}

/// The Nextcloud plugin this host registered, and the transport its profiles use.
pub(crate) struct NextcloudWizard {
    pub(crate) nextcloud: Arc<Nextcloud>,
    pub(crate) transport: &'static str,
}

impl NextcloudWizard {
    pub(crate) async fn run(
        &self,
        engine: &Engine,
        req: DetectNextcloudRequest,
        modes: Option<(String, String)>,
    ) -> Result<DetectNextcloudResponse, ErrorBody> {
        detect_nextcloud(engine, &self.nextcloud, self.transport, req, modes).await
    }
}
