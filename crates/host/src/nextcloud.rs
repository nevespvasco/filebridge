//! The Nextcloud profile wizard for the hosts: the plugin detects and builds the profile
//! (`Nextcloud::set_up`); this turns the ipc request into its input and saves the result.

use std::path::PathBuf;
use std::sync::Arc;

use filebridge_core::{Engine, PreflightView, Profile, TargetSpec};
use filebridge_integration_nextcloud::webdav::{self, NextcloudWebdavProfile};
use filebridge_integration_nextcloud::{
    INTEGRATION_ID, Nextcloud, NextcloudSettings, SetupError, SetupRequest, SshTarget,
};
use filebridge_ipc::{
    DetectNextcloudRequest, DetectNextcloudResponse, DetectStepView, ErrorBody,
    NextcloudWebdavRequest,
};
use filebridge_secrets::{SecretKind, reference};
use filebridge_transport_api::{Backend, Destination, ErrorKind, RemotePath};

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
                    key: None,
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
    pub(crate) async fn webdav(
        &self,
        engine: &Engine,
        req: NextcloudWebdavRequest,
    ) -> Result<DetectNextcloudResponse, ErrorBody> {
        add_webdav(engine, self.transport, req).await
    }

    pub(crate) async fn run(
        &self,
        engine: &Engine,
        req: DetectNextcloudRequest,
        modes: Option<(String, String)>,
    ) -> Result<DetectNextcloudResponse, ErrorBody> {
        detect_nextcloud(engine, &self.nextcloud, self.transport, req, modes).await
    }
}

/// Bytes as decimal GB, for a step's message (the window formats its own numbers).
fn gb(bytes: u64) -> String {
    format!("{:.1} GB", bytes as f64 / 1e9)
}

fn step(id: &str, status: &str, message: impl Into<String>) -> DetectStepView {
    DetectStepView {
        id: id.into(),
        status: status.into(),
        message: message.into(),
        commands: Vec::new(),
    }
}

/// The WebDAV-mode wizard (docs/05): normalise the address, keep the app password in the
/// keyring under a fresh profile id, sign in (list the user's files), look at the folder and the
/// space, and save the profile when sign-in works. A failed sign-in removes the password again.
async fn add_webdav(
    engine: &Engine,
    transport_id: &str,
    req: NextcloudWebdavRequest,
) -> Result<DetectNextcloudResponse, ErrorBody> {
    let address = webdav::parse_address(&req.server).map_err(|m| err("invalid", m))?;
    let server = address.server;
    let user = req.user.trim().to_owned();
    // A WebDAV address copied from Nextcloud names the user's id, which an email or LDAP
    // login is not.
    let user_id = address.user_id.filter(|id| *id != user);
    if user.is_empty() || req.app_password.expose().is_empty() || req.name.trim().is_empty() {
        return Err(err(
            "invalid",
            "a name, the user and an app password are needed",
        ));
    }
    let folder = req.folder.trim().trim_matches('/').to_owned();
    if !folder.is_empty() {
        RemotePath::new(folder.clone()).map_err(|e| err("invalid", e.message))?;
    }
    let url = webdav::dav_url(&server, user_id.as_deref().unwrap_or(&user));
    let mut steps = vec![step("address", "ok", format!("WebDAV address {url}"))];

    let registry = engine.registry();
    let transport = registry
        .transport(transport_id)
        .map_err(|e| err("internal", e.to_string()))?;
    let secrets = registry
        .secrets()
        .map_err(|e| err("keyring", e.to_string()))?;
    let id = uuid::Uuid::new_v4().to_string();
    let pass_ref = reference(&id, SecretKind::AppPassword);
    {
        let (key, value) = (pass_ref.clone(), req.app_password);
        filebridge_secrets::blocking(&secrets, move |s| s.set(&key, value.expose()))
            .await
            .map_err(|e| err("keyring", e.to_string()))?;
    }
    let forget = || async {
        let key = pass_ref.clone();
        let _ = filebridge_secrets::blocking(&secrets, move |s| s.delete(&key)).await;
    };
    let destination = Destination {
        transport: transport_id.into(),
        backend: Backend::Webdav {
            url: url.clone(),
            user: user.clone(),
            password: Some(pass_ref.clone()),
            vendor: Some(webdav::VENDOR.into()),
        },
        root: folder.clone(),
    };
    let files = Destination {
        root: String::new(),
        ..destination.clone()
    };

    // Sign-in: the user's files can be listed.
    match transport.list(&files, None, true).await {
        Ok(_) => steps.push(step("sign-in", "ok", format!("signed in as {user}"))),
        Err(e) => {
            let message = if e.kind == ErrorKind::Auth {
                format!(
                    "Nextcloud refused {user} with this app password ({})",
                    e.message
                )
            } else if e.kind == ErrorKind::NotFound && user_id.is_none() {
                format!(
                    "Nextcloud has no files for {user} at {url}. If you sign in with an email \
                     address or a company account, paste the WebDAV address from Files → \
                     Settings instead of the server's ({})",
                    e.message
                )
            } else {
                format!("could not reach Nextcloud at {server}: {}", e.message)
            };
            steps.push(step("sign-in", "fail", message));
            forget().await;
            return Ok(DetectNextcloudResponse {
                detector: "webdav".into(),
                steps,
                containers: Vec::new(),
                profile: None,
            });
        }
    }
    // The folder: there, or made by the first send.
    if !folder.is_empty() {
        let dir = RemotePath::new(folder.clone()).map_err(|e| err("invalid", e.message))?;
        match transport.list(&files, Some(&dir), true).await {
            Ok(_) => steps.push(step("folder", "ok", format!("folder {folder} found"))),
            Err(e) if e.kind == ErrorKind::NotFound => steps.push(step(
                "folder",
                "warn",
                format!("folder {folder} doesn't exist yet: the first send creates it"),
            )),
            Err(e) => steps.push(step("folder", "warn", e.message)),
        }
    }
    // Space: the account's quota, when Nextcloud reports one.
    match transport.preflight(&files).await {
        Ok(r) => steps.push(step(
            "space",
            "ok",
            match (r.free_bytes, r.total_bytes) {
                (Some(free), Some(total)) => format!("{} free of {}", gb(free), gb(total)),
                (Some(free), None) => format!("{} free", gb(free)),
                _ => "no quota reported".into(),
            },
        )),
        Err(e) => steps.push(step("space", "warn", e.message)),
    }

    let settings = serde_json::to_value(NextcloudSettings::Webdav(NextcloudWebdavProfile {
        server,
        user,
        user_id,
    }))
    .map_err(|e| err("internal", e.to_string()))?;
    let saved = engine
        .save_profile(Profile {
            id,
            name: req.name.trim().to_owned(),
            destination,
            integration: Some(INTEGRATION_ID.into()),
            integration_settings: settings,
            target: TargetSpec::default(),
        })
        .await;
    let profile = match saved {
        Ok(p) => p,
        Err(e) => {
            forget().await;
            return Err(err("invalid", e.to_string()));
        }
    };
    Ok(DetectNextcloudResponse {
        detector: "webdav".into(),
        steps,
        containers: Vec::new(),
        profile: Some(profile),
    })
}
