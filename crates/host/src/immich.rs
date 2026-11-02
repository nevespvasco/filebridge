//! The Immich profile wizard for the hosts (ADR 0020): the plugin checks Immich and builds the
//! settings (`Immich::set_up`); this keeps the secrets in the keyring, checks the destination
//! with this host's transport, and saves the profile.

use std::sync::Arc;

use filebridge_core::{Engine, PreflightView, Profile, TargetSpec};
use filebridge_integration_api::{Check, CheckStatus};
use filebridge_integration_immich::{
    INTEGRATION_ID, Immich, ImmichSettings, SAME_FOLDER, SetupRequest, server_url,
};
use filebridge_ipc::{DetectNextcloudResponse, DetectStepView, ErrorBody, ImmichRequest};
use filebridge_secrets::{SecretKind, reference};
use filebridge_transport_api::{
    Backend, Destination, ErrorKind, ItemOutcome, PROBE_PREFIX, ProgressSink, RemotePath,
    TransferItem, TransferJob, TransferOptions, Transport,
};

fn err(code: &str, message: impl Into<String>) -> ErrorBody {
    ErrorBody {
        code: code.into(),
        message: message.into(),
    }
}

fn step(id: &str, status: &str, message: impl Into<String>) -> DetectStepView {
    DetectStepView {
        id: id.into(),
        status: status.into(),
        message: message.into(),
        commands: Vec::new(),
        hint: None,
    }
}

/// Bytes as decimal GB, for a step's message (the window formats its own numbers).
fn gb(bytes: u64) -> String {
    format!("{:.1} GB", bytes as f64 / 1e9)
}

/// What the probe file says, should anyone open it before it is removed.
const PROBE_TEXT: &[u8] =
    b"filebridge checks that Immich sees this folder; this file is removed right away.\n";

/// Whether Immich sees the destination folder where the settings say: writes a probe file
/// into the destination's root with the host's transport, asks Immich to find it
/// ([`Immich::same_folder`]), and removes that file again, only it ([`Transport::remove_probe`]).
async fn check_same_folder(
    immich: &Immich,
    transport: &dyn Transport,
    destination: &Destination,
    settings: &ImmichSettings,
    key: &str,
) -> Check {
    let warn = |message: String| Check {
        id: SAME_FOLDER.into(),
        status: CheckStatus::Warn,
        message,
    };
    let id = uuid::Uuid::new_v4().simple().to_string();
    let probe = match RemotePath::new(format!("{PROBE_PREFIX}{id}")) {
        Ok(p) => p,
        Err(e) => return warn(e.message),
    };
    let local = std::env::temp_dir().join(format!("filebridge-probe-{id}"));
    let written = async {
        tokio::fs::create_dir_all(&local).await?;
        tokio::fs::write(local.join("probe"), PROBE_TEXT).await
    }
    .await;
    if let Err(e) = written {
        return warn(format!("could not make a test file here: {e}"));
    }
    let job = TransferJob {
        id: format!("probe-{id}"),
        source_roots: vec![local.clone()],
        items: vec![TransferItem {
            root: 0,
            source: "probe".into(),
            dest: probe.clone(),
            size: PROBE_TEXT.len() as u64,
            expected_hash: None,
        }],
        options: TransferOptions::default(),
    };
    let sent = transport
        .transfer(destination, &job, ProgressSink::noop())
        .await;
    let _ = tokio::fs::remove_dir_all(&local).await;
    let failed = match sent {
        Ok(r) => match r.items.into_iter().next() {
            Some(ItemOutcome::Done) => None,
            Some(ItemOutcome::Failed(e)) => Some(e.message),
            None => Some("nothing was sent".into()),
        },
        Err(e) => Some(e.message),
    };
    if let Some(e) = failed {
        return warn(format!(
            "could not write a test file into {} to check that Immich sees it: {e}",
            destination.root
        ));
    }
    let mut check = immich
        .same_folder(settings, key, &destination.root, &probe)
        .await;
    if let Err(e) = transport.remove_probe(destination, &probe).await {
        if check.status == CheckStatus::Ok {
            check.status = CheckStatus::Warn;
        }
        check.message.push_str(&format!(
            "; the test file {probe} stayed in {} ({}): delete it by hand",
            destination.root, e.message
        ));
    }
    check
}

/// The Immich plugin this host registered, and the transport its profiles use.
pub(crate) struct ImmichWizard {
    pub(crate) immich: Arc<Immich>,
    pub(crate) transport: &'static str,
}

impl ImmichWizard {
    /// Keeps the API key (and the SFTP password) in the keyring under a fresh profile id, looks
    /// at the destination folder and its space, checks Immich, and saves the profile when
    /// nothing failed. Otherwise the secrets are removed again.
    pub(crate) async fn run(
        &self,
        engine: &Engine,
        req: ImmichRequest,
    ) -> Result<DetectNextcloudResponse, ErrorBody> {
        let name = req.name.trim().to_owned();
        if name.is_empty() || req.api_key.expose().trim().is_empty() {
            return Err(err("invalid", "a name and the Immich API key are needed"));
        }
        server_url(&req.server).map_err(|m| err("invalid", m))?;
        let root = req.destination.root.trim().to_owned();
        let id = uuid::Uuid::new_v4().to_string();
        let key_ref = reference(&id, SecretKind::ApiKey);
        let pass_ref = reference(&id, SecretKind::AppPassword);
        let password = req.password.filter(|p| !p.expose().is_empty());
        // Only the wizard points the profile at secrets: never a reference the request brought.
        let backend = match req.destination.backend {
            Backend::Sftp {
                host,
                port,
                user,
                key_file,
                known_hosts_file,
                ..
            } => {
                if !root.starts_with('/') {
                    return Err(err(
                        "invalid",
                        "the folder on the SFTP server must be absolute (start with /)",
                    ));
                }
                Backend::Sftp {
                    host: host.trim().to_owned(),
                    port,
                    user: user.map(|u| u.trim().to_owned()).filter(|u| !u.is_empty()),
                    key_file,
                    known_hosts_file,
                    password: password.is_some().then(|| pass_ref.clone()),
                    key: None,
                }
            }
            Backend::Local if password.is_none() => Backend::Local,
            Backend::Local => return Err(err("invalid", "a local folder has no password")),
            _ => {
                return Err(err(
                    "invalid",
                    "Immich reads an external library from a folder: send over SFTP or to a \
                     local folder",
                ));
            }
        };
        if root.is_empty() {
            return Err(err("invalid", "choose the folder the files go to"));
        }
        let destination = Destination {
            transport: self.transport.into(),
            backend,
            root: root.clone(),
        };

        let registry = engine.registry();
        let transport = registry
            .transport(self.transport)
            .map_err(|e| err("internal", e.to_string()))?;
        let secrets = registry
            .secrets()
            .map_err(|e| err("keyring", e.to_string()))?;
        {
            let (k, v) = (key_ref.clone(), req.api_key.clone());
            let (p, pw) = (pass_ref.clone(), password.clone());
            filebridge_secrets::blocking(&secrets, move |s| {
                s.set(&k, v.expose().trim())?;
                match pw {
                    Some(pw) => s.set(&p, pw.expose()),
                    None => Ok(()),
                }
            })
            .await
            .map_err(|e| err("keyring", e.to_string()))?;
        }
        let forget = || async {
            let (k, p) = (key_ref.clone(), pass_ref.clone());
            let _ = filebridge_secrets::blocking(&secrets, move |s| {
                let _ = s.delete(&k);
                s.delete(&p)
            })
            .await;
        };
        let refused = |steps: Vec<DetectStepView>, containers: Vec<String>| {
            Ok(DetectNextcloudResponse {
                detector: INTEGRATION_ID.into(),
                steps,
                containers,
                profile: None,
                server_setup: Vec::new(),
            })
        };

        // The destination folder: there, or made by the first send.
        let mut steps = Vec::new();
        let mut folder_there = false;
        match transport.list(&destination, None, true).await {
            Ok(_) => {
                folder_there = true;
                steps.push(step("destination", "ok", format!("folder {root} found")));
            }
            Err(e) if e.kind == ErrorKind::NotFound => steps.push(step(
                "destination",
                "warn",
                format!("folder {root} doesn't exist yet: the first send creates it"),
            )),
            Err(e) => {
                let message = if e.kind == ErrorKind::Auth {
                    format!("the server refused the sign-in ({})", e.message)
                } else {
                    format!("could not reach the destination: {}", e.message)
                };
                steps.push(step("destination", "fail", message));
                forget().await;
                return refused(steps, Vec::new());
            }
        }
        match transport.preflight(&destination).await {
            Ok(r) => steps.push(step(
                "space",
                "ok",
                match (r.free_bytes, r.total_bytes) {
                    (Some(free), Some(total)) => format!("{} free of {}", gb(free), gb(total)),
                    (Some(free), None) => format!("{} free", gb(free)),
                    _ => "no free space reported".into(),
                },
            )),
            Err(e) => steps.push(step("space", "warn", e.message)),
        }

        let setup = self
            .immich
            .set_up(&SetupRequest {
                server: &req.server,
                api_key: req.api_key.expose(),
                api_key_ref: key_ref.clone(),
                library_id: req.library_id.as_deref(),
                import_path: req.import_path.as_deref(),
                destination_root: &root,
            })
            .await;
        let setup = match setup {
            Ok(s) => s,
            Err(m) => {
                forget().await;
                return Err(err("invalid", m));
            }
        };
        steps.extend(setup.checks.iter().map(|c| {
            step(
                &c.id,
                PreflightView::status_text(c.status),
                c.message.clone(),
            )
        }));
        let libraries = setup
            .candidates
            .iter()
            .map(|l| format!("{} {} ({})", l.id, l.name, l.import_paths.join(", ")))
            .collect();
        let Some(settings) = setup.settings else {
            forget().await;
            return refused(steps, libraries);
        };
        let same = if folder_there {
            check_same_folder(
                &self.immich,
                transport.as_ref(),
                &destination,
                &settings,
                req.api_key.expose().trim(),
            )
            .await
        } else {
            Check {
                id: SAME_FOLDER.into(),
                status: CheckStatus::Warn,
                message: format!(
                    "{root} doesn't exist yet, so filebridge couldn't check that Immich sees it \
                     as {}: the first send creates it",
                    settings.import_path
                ),
            }
        };
        let same_failed = same.status == CheckStatus::Fail;
        steps.push(step(
            &same.id,
            PreflightView::status_text(same.status),
            same.message,
        ));
        if same_failed {
            forget().await;
            return refused(steps, libraries);
        }
        let settings =
            serde_json::to_value(settings).map_err(|e| err("internal", e.to_string()))?;
        let saved = engine
            .save_profile(Profile {
                id,
                name,
                destination,
                integration: Some(INTEGRATION_ID.into()),
                integration_settings: settings,
                target: TargetSpec::default(),
            })
            .await;
        match saved {
            Ok(profile) => Ok(DetectNextcloudResponse {
                detector: INTEGRATION_ID.into(),
                steps,
                containers: libraries,
                profile: Some(profile),
                server_setup: Vec::new(),
            }),
            Err(e) => {
                forget().await;
                Err(err("invalid", e.to_string()))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use filebridge_core::{Event, JobState, SendOptions};
    use filebridge_integration_immich::fake::MockImmich;
    use filebridge_ipc::{Operations, Password};
    use filebridge_secrets::{MemoryStore, SecretStore};

    use super::*;
    use crate::{Access, Host, HostOptions, TransportChoice};

    const KEY: &str = "imm-key-0123456789abcdef";
    const LIB: &str = "6f1c2a4e-0b7d-4c55-9a8e-3d2f1b0c9e77";

    async fn host(dir: &std::path::Path) -> (Host, Arc<MemoryStore>) {
        let secrets = Arc::new(MemoryStore::default());
        let host = Host::open_with_secrets(
            &HostOptions {
                data_dir: Some(dir.join("data")),
                cache_dir: None,
                rclone: None,
                transport: TransportChoice::Fake,
            },
            Access::Exclusive,
            Some(secrets.clone()),
        )
        .await
        .unwrap();
        (host, secrets)
    }

    fn request(server: &str, root: &std::path::Path, key: &str) -> ImmichRequest {
        ImmichRequest {
            name: "Fotos".into(),
            destination: Destination {
                transport: "anything".into(),
                backend: Backend::Local,
                root: root.to_string_lossy().into_owned(),
            },
            password: None,
            server: server.into(),
            api_key: Password::new(key),
            library_id: None,
            import_path: Some("/mnt/photos/Holidays".into()),
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn the_wizard_saves_a_profile_whose_sends_scan_the_library() {
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("photos");
        let src = dir.path().join("Holidays");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("a.jpg"), b"jpeg").unwrap();
        let mock = MockImmich::start(KEY).await;
        mock.add_library(LIB, "Fotos", &["/mnt/photos"]);
        // The scan is over at the first look, so the send doesn't wait for the next.
        mock.set_scan_polls(1);
        let (host, secrets) = host(dir.path()).await;
        let ops = host.operations();

        let res = ops
            .add_immich(request(mock.url(), &dest, KEY))
            .await
            .unwrap();
        let ids: Vec<_> = res
            .steps
            .iter()
            .map(|s| (s.id.as_str(), s.status.as_str()))
            .collect();
        assert_eq!(
            ids,
            [
                ("destination", "warn"),
                ("space", "ok"),
                ("server", "ok"),
                ("api-key", "ok"),
                ("library", "ok"),
                ("import-path", "ok"),
                ("readable", "ok"),
                // The folder isn't there yet: nothing to probe.
                ("same-folder", "warn"),
            ],
            "{:?}",
            res.steps
        );
        let profile = res.profile.expect("saved");
        assert_eq!(profile.integration.as_deref(), Some(INTEGRATION_ID));
        assert_eq!(profile.destination.transport, "fake");
        let key_ref = reference(&profile.id, SecretKind::ApiKey);
        assert_eq!(secrets.get(&key_ref).unwrap().0.as_str(), KEY);
        let shown = serde_json::to_string(&profile).unwrap();
        assert!(!shown.contains(KEY), "{shown}");

        // A send to it ends with a scan of the library.
        let mut events = host.engine.subscribe();
        let job = ops
            .create_job(
                vec![src.to_string_lossy().into_owned()],
                profile.id.clone(),
                SendOptions::default(),
            )
            .await
            .unwrap();
        ops.analyse(job.clone()).await.unwrap();
        ops.start(job.clone()).await.unwrap();
        let state = tokio::time::timeout(Duration::from_secs(20), async {
            loop {
                if let Ok(Event::JobFinished { job_id, state, .. }) = events.recv().await
                    && job_id == job
                {
                    return state;
                }
            }
        })
        .await
        .expect("the job finishes");
        assert_eq!(state, JobState::Completed);
        assert!(dest.join("Holidays/a.jpg").exists());
        assert_eq!(mock.scans(), [format!("/api/libraries/{LIB}/scan")]);

        // Deleting the profile takes its key out of the keyring.
        ops.delete_profile(profile.id.clone()).await.unwrap();
        assert!(secrets.get(&key_ref).is_err());
    }

    /// The names in `dir`.
    fn names(dir: &std::path::Path) -> Vec<String> {
        let mut v: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        v.sort();
        v
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_test_file_shows_whether_immich_sees_the_destination() {
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("photos");
        std::fs::create_dir_all(&dest).unwrap();
        std::fs::write(dest.join("kept.jpg"), b"jpeg").unwrap();
        let (first, _) = host(dir.path()).await;
        let ops = first.operations();

        // Immich's container mounts the destination at /mnt/photos/Holidays.
        let mock = MockImmich::start(KEY).await;
        mock.add_library(LIB, "Fotos", &["/mnt/photos"]);
        mock.mount("/mnt/photos/Holidays", &dest);
        let res = ops
            .add_immich(request(mock.url(), &dest, KEY))
            .await
            .unwrap();
        let same = res.steps.iter().find(|s| s.id == "same-folder").unwrap();
        assert_eq!(same.status, "ok", "{}", same.message);
        assert!(res.profile.is_some());
        assert_eq!(names(&dest), ["kept.jpg"], "only the test file is removed");
        let validated: Vec<_> = mock
            .requests()
            .into_iter()
            .filter(|r| r.path.ends_with("/validate") && r.body.contains(PROBE_PREFIX))
            .collect();
        assert_eq!(validated.len(), 1);

        // It mounts another folder there: refused, nothing saved, the test file removed.
        let elsewhere = dir.path().join("elsewhere");
        std::fs::create_dir_all(&elsewhere).unwrap();
        let mock = MockImmich::start(KEY).await;
        mock.add_library(LIB, "Fotos", &["/mnt/photos"]);
        mock.mount("/mnt/photos/Holidays", &elsewhere);
        let (second, secrets) = host(&dir.path().join("second")).await;
        let ops = second.operations();
        let res = ops
            .add_immich(request(mock.url(), &dest, KEY))
            .await
            .unwrap();
        let same = res.steps.iter().find(|s| s.id == "same-folder").unwrap();
        assert_eq!(same.status, "fail", "{}", same.message);
        assert!(res.profile.is_none());
        assert!(secrets.is_empty(), "the key went again");
        assert_eq!(names(&dest), ["kept.jpg"]);
        assert!(ops.list_profiles().await.unwrap().is_empty());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_refused_key_saves_nothing_and_keeps_no_secret() {
        let dir = tempfile::tempdir().unwrap();
        let mock = MockImmich::start(KEY).await;
        mock.add_library(LIB, "Fotos", &["/mnt/photos"]);
        let (host, secrets) = host(dir.path()).await;
        let ops = host.operations();
        let mut req = request(mock.url(), &dir.path().join("photos"), "wrong-key");
        req.password = None;
        let res = ops.add_immich(req).await.unwrap();
        assert!(res.profile.is_none());
        let key = res.steps.iter().find(|s| s.id == "api-key").unwrap();
        assert_eq!(key.status, "fail");
        assert!(ops.list_profiles().await.unwrap().is_empty());
        assert!(secrets.is_empty());

        // Plain http to another machine is refused before anything is stored.
        let err = ops
            .add_immich(request(
                "http://192.0.2.10:2283",
                &dir.path().join("photos"),
                KEY,
            ))
            .await
            .unwrap_err();
        assert_eq!(err.code, "invalid");
        assert!(secrets.is_empty());
    }
}
