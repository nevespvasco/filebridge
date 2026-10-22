//! filebridge's own SSH key for a profile, and "Remove access" (ADR 0012, docs/07).
//!
//! Installing: an Ed25519 key is generated, its public line added with `restrict` to the
//! `authorized_keys` of each SSH user the profile signs in as (through the access it has today),
//! the private key stored in the OS keyring, and only once a sign-in with the new key has worked
//! (and rclone has listed the destination with it) is the profile switched to it. Any failure
//! undoes what was done.
//!
//! Removing: the line is deleted from each server (signed in with the key itself), the key is
//! deleted from the keyring, the profile goes back to its key file or SSH agent, and a last
//! sign-in with the old key, held in memory, must be refused.

use std::sync::Arc;

use filebridge_core::{Engine, Profile};
use filebridge_integration_nextcloud::ssh::{NewKey, generate_key};
use filebridge_integration_nextcloud::{NextcloudSettings, ShellConnector, SshTarget};
use filebridge_ipc::{AccessReport, DetectStepView, ErrorBody};
use filebridge_secrets::{KeyringRef, SecretKind, SecretStore, reference};
use filebridge_transport_api::Backend;
use russh::keys::PrivateKey;

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
    }
}

/// The comment that marks filebridge's line for one profile in `authorized_keys`.
fn comment(profile_id: &str) -> String {
    format!("filebridge-{profile_id}")
}

/// Adds `line` to `~/.ssh/authorized_keys` once, creating the folder and file private, and
/// first ending a last line that lacks its newline (or the new line would join it).
pub(crate) fn install_script(line: &str) -> String {
    let q = filebridge_integration_nextcloud::quote::quote(line);
    format!(
        "umask 077 && mkdir -p ~/.ssh && f=~/.ssh/authorized_keys && touch \"$f\" && \
         {{ grep -qxF {q} \"$f\" || {{ if [ -s \"$f\" ] && [ -n \"$(tail -c 1 \"$f\")\" ]; then \
         echo >> \"$f\"; fi; printf '%s\\n' {q} >> \"$f\"; }}; }}"
    )
}

/// Removes every line of filebridge's key for `profile_id` from `~/.ssh/authorized_keys`, and
/// nothing else. Rewritten in place (`cat >`), so the file keeps its owner and mode.
pub(crate) fn remove_script(profile_id: &str) -> String {
    let pattern = format!(" {}$", comment(profile_id));
    let pattern = filebridge_integration_nextcloud::quote::quote(&pattern);
    format!(
        "f=~/.ssh/authorized_keys; [ -f \"$f\" ] || exit 0; t=$(mktemp) || exit 1; \
         grep -v -e {pattern} \"$f\" > \"$t\"; cat \"$t\" > \"$f\" && rm -f \"$t\""
    )
}

/// The SSH sign-ins a profile makes: its SFTP user, and for Nextcloud's direct mode the SSH user
/// that runs `occ` when it differs.
fn targets(profile: &Profile) -> Result<Vec<SshTarget>, ErrorBody> {
    let Backend::Sftp {
        host,
        port,
        user,
        key_file,
        known_hosts_file,
        ..
    } = &profile.destination.backend
    else {
        return Err(err(
            "invalid",
            "only destinations reached over SSH (SFTP, Nextcloud direct mode) have an SSH key",
        ));
    };
    let user = user
        .clone()
        .ok_or_else(|| err("invalid", "the profile names no SSH user"))?;
    let mut out = vec![SshTarget {
        host: host.clone(),
        port: port.unwrap_or(22),
        user,
        key_file: key_file.clone(),
        known_hosts_file: known_hosts_file.clone(),
        key: None,
    }];
    if let Ok(NextcloudSettings::Direct(nc)) =
        serde_json::from_value::<NextcloudSettings>(profile.integration_settings.clone())
        && !out
            .iter()
            .any(|t| t.host == nc.ssh.host && t.port == nc.ssh.port && t.user == nc.ssh.user)
    {
        out.push(SshTarget {
            key: None,
            ..nc.ssh.clone()
        });
    }
    Ok(out)
}

/// The profile signing in with `key` (or, with `None`, with its key file or agent again).
fn with_key(profile: &Profile, key: Option<KeyringRef>) -> Result<Profile, ErrorBody> {
    let mut p = profile.clone();
    if let Backend::Sftp { key: slot, .. } = &mut p.destination.backend {
        *slot = key.clone();
    }
    if let Ok(NextcloudSettings::Direct(mut nc)) =
        serde_json::from_value::<NextcloudSettings>(p.integration_settings.clone())
    {
        nc.ssh.key = key;
        p.integration_settings = serde_json::to_value(NextcloudSettings::Direct(nc))
            .map_err(|e| err("internal", e.to_string()))?;
    }
    Ok(p)
}

async fn keyring<T: Send + 'static>(
    store: &Arc<dyn SecretStore>,
    f: impl FnOnce(&dyn SecretStore) -> filebridge_secrets::Result<T> + Send + 'static,
) -> Result<T, ErrorBody> {
    filebridge_secrets::blocking(store, f)
        .await
        .map_err(|e| err("keyring", e.to_string()))
}

fn who(t: &SshTarget) -> String {
    format!("{}@{}", t.user, t.host)
}

/// Generates, installs and switches the profile to filebridge's own key.
pub(crate) async fn install(
    engine: &Engine,
    connector: &dyn ShellConnector,
    profile_id: &str,
) -> Result<AccessReport, ErrorBody> {
    let profile = engine
        .get_profile(profile_id)
        .await
        .map_err(|e| err("not_found", e.to_string()))?;
    if matches!(
        profile.destination.backend,
        Backend::Sftp { key: Some(_), .. }
    ) {
        return Err(err(
            "invalid",
            "this destination already uses filebridge's own key",
        ));
    }
    let targets = targets(&profile)?;
    let secrets = engine
        .registry()
        .secrets()
        .map_err(|e| err("keyring", e.to_string()))?;
    let NewKey { key, pem, public } =
        generate_key(&comment(profile_id)).map_err(|e| err("internal", e.message))?;
    let line = format!("restrict {public}");
    let mut steps = vec![step(
        "generate",
        "ok",
        "a new Ed25519 key, made on this computer",
    )];
    let key_ref = reference(profile_id, SecretKind::SshKey);

    let mut installed: Vec<SshTarget> = Vec::new();
    let failed = async |steps: &mut Vec<DetectStepView>, installed: &[SshTarget]| {
        // Undo: the lines added so far, with the access the profile had, and the secret.
        for t in installed {
            if let Ok(shell) = connector.connect(t).await {
                let _ = shell.run(&remove_script(profile_id)).await;
            }
        }
        let r = key_ref.clone();
        let _ = keyring(&secrets, move |s| s.delete(&r)).await;
        steps.push(step(
            "undone",
            "warn",
            "nothing was changed: the key was removed again",
        ));
    };

    for t in &targets {
        let outcome = match connector.connect(t).await {
            Ok(shell) => shell
                .run(&install_script(&line))
                .await
                .map_err(|e| e.message)
                .and_then(|o| {
                    if o.success() {
                        Ok(())
                    } else {
                        Err(o.short_error())
                    }
                }),
            Err(e) => Err(e.message),
        };
        match outcome {
            Ok(()) => {
                steps.push(step(
                    "install",
                    "ok",
                    format!("added to {}'s authorized_keys, restricted", who(t)),
                ));
                installed.push(t.clone());
            }
            Err(e) => {
                steps.push(step("install", "fail", format!("{}: {e}", who(t))));
                failed(&mut steps, &installed).await;
                return Ok(AccessReport {
                    steps,
                    profile: None,
                });
            }
        }
    }

    let (r, value) = (key_ref.clone(), pem.to_string());
    if let Err(e) = keyring(&secrets, move |s| s.set(&r, &value)).await {
        steps.push(step("keyring", "fail", e.message));
        failed(&mut steps, &installed).await;
        return Ok(AccessReport {
            steps,
            profile: None,
        });
    }
    steps.push(step(
        "keyring",
        "ok",
        "the private key is in the system keyring",
    ));

    for t in &targets {
        match connector.key_accepted(t, key.clone()).await {
            Ok(true) => steps.push(step(
                "sign-in",
                "ok",
                format!("{} signs in with the new key", who(t)),
            )),
            outcome => {
                let why = match outcome {
                    Err(e) => e.message,
                    _ => "the server refused it".into(),
                };
                steps.push(step("sign-in", "fail", format!("{}: {why}", who(t))));
                failed(&mut steps, &installed).await;
                return Ok(AccessReport {
                    steps,
                    profile: None,
                });
            }
        }
    }

    let updated = with_key(&profile, Some(key_ref.clone()))?;
    let transport = engine
        .registry()
        .transport(&updated.destination.transport)
        .map_err(|e| err("internal", e.to_string()))?;
    if let Err(e) = transport.list(&updated.destination, None, true).await {
        steps.push(step("transfer", "fail", e.message));
        failed(&mut steps, &installed).await;
        return Ok(AccessReport {
            steps,
            profile: None,
        });
    }
    steps.push(step("transfer", "ok", "files can be sent with the new key"));

    let saved = engine
        .save_profile(updated)
        .await
        .map_err(|e| err("invalid", e.to_string()))?;
    steps.push(step(
        "saved",
        "ok",
        "the destination now uses filebridge's own key; your own key is no longer used",
    ));
    Ok(AccessReport {
        steps,
        profile: Some(saved),
    })
}

/// Deletes filebridge's key from the servers and the keyring, then proves it no longer works.
pub(crate) async fn remove(
    engine: &Engine,
    connector: &dyn ShellConnector,
    profile_id: &str,
) -> Result<AccessReport, ErrorBody> {
    let profile = engine
        .get_profile(profile_id)
        .await
        .map_err(|e| err("not_found", e.to_string()))?;
    let Backend::Sftp {
        key: Some(key_ref), ..
    } = &profile.destination.backend
    else {
        return Err(err(
            "invalid",
            "this destination doesn't use filebridge's own key",
        ));
    };
    let key_ref = key_ref.clone();
    let targets = targets(&profile)?;
    let secrets = engine
        .registry()
        .secrets()
        .map_err(|e| err("keyring", e.to_string()))?;
    let mut steps = Vec::new();
    // Held in memory for the proof at the end.
    let key: Option<Arc<PrivateKey>> = {
        let r = key_ref.clone();
        keyring(&secrets, move |s| s.get(&r))
            .await
            .ok()
            .and_then(|pem| russh::keys::decode_secret_key(pem.0.as_str(), None).ok())
            .map(Arc::new)
    };
    if key.is_none() {
        steps.push(step(
            "keyring",
            "warn",
            "the key was not in the keyring any more; removing it from the servers with your own access",
        ));
    }

    for t in &targets {
        // With the key itself; if it no longer signs in, with the profile's own access.
        let shell = match &key {
            Some(k) => match connector.connect_with_key(t, k.clone()).await {
                Ok(s) => Ok(s),
                Err(_) => connector.connect(t).await,
            },
            None => connector.connect(t).await,
        };
        let outcome = match shell {
            Ok(shell) => shell
                .run(&remove_script(profile_id))
                .await
                .map_err(|e| e.message)
                .and_then(|o| {
                    if o.success() {
                        Ok(())
                    } else {
                        Err(o.short_error())
                    }
                }),
            Err(e) => Err(e.message),
        };
        match outcome {
            Ok(()) => steps.push(step(
                "remove",
                "ok",
                format!("removed from {}'s authorized_keys", who(t)),
            )),
            Err(e) => {
                steps.push(step("remove", "fail", format!("{}: {e}", who(t))));
                return Ok(AccessReport {
                    steps,
                    profile: None,
                });
            }
        }
    }

    let r = key_ref.clone();
    keyring(&secrets, move |s| s.delete(&r)).await?;
    steps.push(step("keyring", "ok", "deleted from the system keyring"));
    let saved = engine
        .save_profile(with_key(&profile, None)?)
        .await
        .map_err(|e| err("invalid", e.to_string()))?;
    steps.push(step(
        "saved",
        "ok",
        "the destination signs in with your key file or SSH agent again",
    ));

    if let Some(k) = key {
        for t in &targets {
            steps.push(match connector.key_accepted(t, k.clone()).await {
                Ok(false) => step(
                    "proof",
                    "ok",
                    format!("{} refuses the old key: access is gone", who(t)),
                ),
                Ok(true) => step(
                    "proof",
                    "fail",
                    format!(
                        "{} still accepts the old key: check its other authorized_keys files",
                        who(t)
                    ),
                ),
                Err(e) => step(
                    "proof",
                    "warn",
                    format!("could not check {}: {}", who(t), e.message),
                ),
            });
        }
    }
    Ok(AccessReport {
        steps,
        profile: Some(saved),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    /// Runs a script as `sh` would on the server, with `home` as `~`.
    fn run(script: &str, home: &std::path::Path) -> bool {
        Command::new("sh")
            .args(["-c", script])
            .env("HOME", home)
            .status()
            .unwrap()
            .success()
    }

    #[test]
    fn installing_twice_adds_one_line_and_keeps_the_others() {
        let home = tempfile::tempdir().unwrap();
        let ssh = home.path().join(".ssh");
        std::fs::create_dir(&ssh).unwrap();
        // An existing key whose line has no final newline.
        std::fs::write(
            ssh.join("authorized_keys"),
            "ssh-ed25519 AAAAmine alice@laptop",
        )
        .unwrap();
        let line = "restrict ssh-ed25519 AAAAnew filebridge-p-1";
        assert!(run(&install_script(line), home.path()));
        assert!(run(&install_script(line), home.path()));
        let text = std::fs::read_to_string(ssh.join("authorized_keys")).unwrap();
        assert_eq!(text, format!("ssh-ed25519 AAAAmine alice@laptop\n{line}\n"));
    }

    #[test]
    fn installing_creates_a_private_ssh_folder() {
        let home = tempfile::tempdir().unwrap();
        assert!(run(
            &install_script("restrict ssh-ed25519 AAAA filebridge-p"),
            home.path()
        ));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode =
                |p: &std::path::Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode(&home.path().join(".ssh")), 0o700);
            assert_eq!(mode(&home.path().join(".ssh/authorized_keys")), 0o600);
        }
    }

    #[test]
    fn removing_takes_only_that_profiles_lines() {
        let home = tempfile::tempdir().unwrap();
        let ssh = home.path().join(".ssh");
        std::fs::create_dir(&ssh).unwrap();
        let keys = ssh.join("authorized_keys");
        std::fs::write(
            &keys,
            "ssh-ed25519 AAAAmine alice@laptop\n\
             restrict ssh-ed25519 AAAAone filebridge-p-1\n\
             restrict ssh-ed25519 AAAAtwo filebridge-p-12\n\
             ssh-rsa AAAAtheirs filebridge-p-1-notes and more\n",
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&keys, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
        assert!(run(&remove_script("p-1"), home.path()));
        let text = std::fs::read_to_string(&keys).unwrap();
        assert_eq!(
            text,
            "ssh-ed25519 AAAAmine alice@laptop\n\
             restrict ssh-ed25519 AAAAtwo filebridge-p-12\n\
             ssh-rsa AAAAtheirs filebridge-p-1-notes and more\n"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&keys).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        // Removing again, or with no file at all, is fine.
        assert!(run(&remove_script("p-1"), home.path()));
        let empty = tempfile::tempdir().unwrap();
        assert!(run(&remove_script("p-1"), empty.path()));
    }

    /// Pretend SSH servers: one home per user under a temp folder; commands run there with
    /// `sh`, and a key signs in when its line is in that home's `authorized_keys`.
    struct LocalServers(tempfile::TempDir);

    struct LocalShell(std::path::PathBuf);

    impl LocalServers {
        fn home(&self, user: &str) -> std::path::PathBuf {
            let h = self.0.path().join(user);
            std::fs::create_dir_all(&h).unwrap();
            h
        }

        fn authorized(&self, user: &str) -> String {
            std::fs::read_to_string(self.home(user).join(".ssh/authorized_keys"))
                .unwrap_or_default()
        }
    }

    #[async_trait::async_trait]
    impl filebridge_integration_nextcloud::RemoteShell for LocalShell {
        async fn run(
            &self,
            cmd: &str,
        ) -> filebridge_integration_api::Result<filebridge_integration_nextcloud::Output> {
            let out = Command::new("sh")
                .args(["-c", cmd])
                .env("HOME", &self.0)
                .output()
                .unwrap();
            Ok(filebridge_integration_nextcloud::Output {
                status: out.status.code().unwrap_or(-1),
                stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
                stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
            })
        }
    }

    #[async_trait::async_trait]
    impl ShellConnector for LocalServers {
        async fn connect(
            &self,
            target: &SshTarget,
        ) -> filebridge_integration_api::Result<
            Arc<dyn filebridge_integration_nextcloud::RemoteShell>,
        > {
            Ok(Arc::new(LocalShell(self.home(&target.user))))
        }

        async fn connect_with_key(
            &self,
            target: &SshTarget,
            key: Arc<PrivateKey>,
        ) -> filebridge_integration_api::Result<
            Arc<dyn filebridge_integration_nextcloud::RemoteShell>,
        > {
            if self.key_accepted(target, key).await? {
                self.connect(target).await
            } else {
                Err(filebridge_integration_api::IntegrationError::fatal(
                    "refused",
                ))
            }
        }

        async fn key_accepted(
            &self,
            target: &SshTarget,
            key: Arc<PrivateKey>,
        ) -> filebridge_integration_api::Result<bool> {
            let public = key.public_key().to_openssh().unwrap();
            let base64 = public.split_whitespace().nth(1).unwrap().to_owned();
            Ok(self
                .authorized(&target.user)
                .lines()
                .any(|l| l.split_whitespace().any(|w| w == base64)))
        }
    }

    async fn engine_with_profile(
        root: &std::path::Path,
    ) -> (Arc<Engine>, Arc<filebridge_secrets::MemoryStore>, String) {
        let secrets = Arc::new(filebridge_secrets::MemoryStore::default());
        let mut registry = filebridge_core::Registry::new();
        registry.add_transport(Arc::new(filebridge_transport_fake::FakeTransport::default()));
        registry.set_secrets(secrets.clone());
        let engine = Engine::new(
            filebridge_core::Store::open_in_memory().unwrap(),
            registry,
            filebridge_core::EngineOptions::default(),
        );
        let p = engine
            .save_profile(Profile {
                id: String::new(),
                name: "NAS".into(),
                destination: filebridge_transport_api::Destination {
                    transport: "fake".into(),
                    backend: Backend::Sftp {
                        host: "nas.example.lan".into(),
                        port: None,
                        user: Some("alice".into()),
                        key_file: None,
                        known_hosts_file: None,
                        password: None,
                        key: None,
                    },
                    root: root.to_string_lossy().into_owned(),
                },
                integration: None,
                integration_settings: serde_json::json!({}),
                target: filebridge_core::TargetSpec::default(),
            })
            .await
            .unwrap();
        (engine, secrets, p.id)
    }

    #[tokio::test]
    async fn install_then_remove_access() {
        let dest = tempfile::tempdir().unwrap();
        let (engine, secrets, id) = engine_with_profile(dest.path()).await;
        let servers = LocalServers(tempfile::tempdir().unwrap());
        // Someone else's key, which must survive both operations.
        let ssh = servers.home("alice").join(".ssh");
        std::fs::create_dir_all(&ssh).unwrap();
        std::fs::write(
            ssh.join("authorized_keys"),
            "ssh-ed25519 AAAAmine alice@laptop\n",
        )
        .unwrap();

        let report = install(&engine, &servers, &id).await.unwrap();
        let fails: Vec<_> = report.steps.iter().filter(|s| s.status != "ok").collect();
        assert!(fails.is_empty(), "{fails:?}");
        let saved = report.profile.unwrap();
        let key_ref = reference(&id, SecretKind::SshKey);
        assert!(
            matches!(&saved.destination.backend, Backend::Sftp { key: Some(r), .. } if *r == key_ref)
        );
        let pem = secrets.get(&key_ref).unwrap();
        assert!(pem.0.contains("OPENSSH PRIVATE KEY"));
        let lines = servers.authorized("alice");
        assert!(lines.starts_with("ssh-ed25519 AAAAmine alice@laptop\n"));
        let ours = lines
            .lines()
            .find(|l| l.ends_with(&format!(" filebridge-{id}")))
            .unwrap();
        assert!(ours.starts_with("restrict ssh-ed25519 "), "{ours}");
        assert!(
            install(&engine, &servers, &id).await.is_err(),
            "a second key is refused"
        );

        let report = remove(&engine, &servers, &id).await.unwrap();
        let proof = report.steps.iter().find(|s| s.id == "proof").unwrap();
        assert_eq!(proof.status, "ok", "{report:?}");
        assert_eq!(
            servers.authorized("alice"),
            "ssh-ed25519 AAAAmine alice@laptop\n"
        );
        assert!(secrets.get(&key_ref).is_err());
        let p = engine.get_profile(&id).await.unwrap();
        assert!(matches!(
            p.destination.backend,
            Backend::Sftp { key: None, .. }
        ));
    }

    #[tokio::test]
    async fn a_failed_install_leaves_nothing_behind() {
        // The destination folder doesn't exist: the last check (listing it) fails.
        let missing = tempfile::tempdir().unwrap().path().join("nope");
        let (engine, secrets, id) = engine_with_profile(&missing).await;
        let servers = LocalServers(tempfile::tempdir().unwrap());
        let report = install(&engine, &servers, &id).await.unwrap();
        assert!(report.profile.is_none());
        assert!(
            report
                .steps
                .iter()
                .any(|s| s.id == "transfer" && s.status == "fail")
        );
        assert!(report.steps.iter().any(|s| s.id == "undone"));
        assert!(
            !servers.authorized("alice").contains("filebridge-"),
            "line removed again"
        );
        assert!(secrets.get(&reference(&id, SecretKind::SshKey)).is_err());
        let p = engine.get_profile(&id).await.unwrap();
        assert!(matches!(
            p.destination.backend,
            Backend::Sftp { key: None, .. }
        ));
    }
}
