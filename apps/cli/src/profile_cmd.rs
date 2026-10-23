//! `filebridge profile …`: destinations.

use std::path::PathBuf;

use anyhow::{Result, bail};
use clap::Subcommand;
use filebridge_core::{Family, PreflightView, Profile, TargetSpec};
use filebridge_integration_nextcloud::NextcloudDirectProfile;
use filebridge_ipc::DetectNextcloudRequest;
use filebridge_transport_api::{Backend, Destination};
use filebridge_transport_rclone::RcloneTransport;

use crate::app::{App, Global};
use filebridge_host::Access;
use filebridge_ipc::Operations;

#[derive(clap::Args, Clone)]
pub struct SshArgs {
    /// SSH host (name from ~/.ssh/config is not resolved; give the address).
    #[arg(long)]
    host: String,
    #[arg(long, default_value_t = 22)]
    port: u16,
    #[arg(long, default_value = "root")]
    user: String,
    /// Private key file; when absent the SSH agent is used.
    #[arg(long)]
    key_file: Option<PathBuf>,
    /// Defaults to ~/.ssh/known_hosts. Host keys are always verified.
    #[arg(long)]
    known_hosts: Option<PathBuf>,
}

impl SshArgs {
    fn backend(&self) -> Backend {
        Backend::Sftp {
            host: self.host.clone(),
            port: Some(self.port),
            user: Some(self.user.clone()),
            key_file: self.key_file.clone(),
            known_hosts_file: self.known_hosts.clone(),
            password: None,
            key: None,
        }
    }
}

#[derive(Subcommand)]
pub enum ProfileCmd {
    /// A folder on this machine (or a mounted disk).
    AddLocal {
        #[arg(long)]
        name: String,
        #[arg(long)]
        root: PathBuf,
        /// Use Windows-compatible names (forbidden characters, trailing dots, CON/NUL…).
        #[arg(long)]
        windows: bool,
    },
    /// A folder on an SFTP server.
    AddSftp {
        #[arg(long)]
        name: String,
        #[command(flatten)]
        ssh: SshArgs,
        /// Absolute folder on the server.
        #[arg(long)]
        root: String,
        #[arg(long)]
        windows: bool,
    },
    /// Nextcloud over its own WebDAV API with an app password (the default mode; nothing to set
    /// up on the server). The app password is asked without echo, or read from stdin.
    AddNextcloudWebdav {
        #[arg(long)]
        name: String,
        /// The Nextcloud address, e.g. `cloud.example.com` (https), or its WebDAV address.
        #[arg(long)]
        server: String,
        /// The Nextcloud user.
        #[arg(long)]
        user: String,
        /// Folder inside that user's files, e.g. `Backup` (default: the files root).
        #[arg(long, default_value = "")]
        folder: String,
    },
    /// Immich (ADR 0020): files go over SFTP (or to a local folder with --local) into a folder of
    /// an Immich external library, which is scanned after each send. The API key (an
    /// administrator's, Account settings → API keys) is asked without echo, or read from stdin.
    AddImmich {
        #[arg(long)]
        name: String,
        /// The Immich address, e.g. `photos.example.com` (https).
        #[arg(long)]
        server: String,
        /// The destination folder: absolute on the SFTP server, or on this machine with --local.
        #[arg(long)]
        root: String,
        /// Send to a folder on this machine instead of over SFTP.
        #[arg(long, conflicts_with = "host")]
        local: bool,
        #[command(flatten)]
        ssh: Option<SshArgs>,
        /// The external library's id (default: the one library that imports the folder).
        #[arg(long)]
        library: Option<String>,
        /// The folder as Immich sees it, when Immich runs in a container that mounts it
        /// elsewhere, e.g. `/mnt/photos` (default: the same path as --root).
        #[arg(long)]
        import_path: Option<String>,
    },
    /// Nextcloud direct mode, over SSH to the Proxmox node (Nextcloud in an LXC) or to the
    /// machine Nextcloud runs on: detects the container, data folder, `occ` and owner, then
    /// saves the profile. Runs read-only commands on the host.
    AddNextcloud {
        #[arg(long)]
        name: String,
        #[command(flatten)]
        ssh: SshArgs,
        /// Nextcloud user whose files receive the sends.
        #[arg(long)]
        nc_user: String,
        /// Folder inside that user's files, e.g. `Backup` (default: the files root).
        #[arg(long, default_value = "")]
        folder: String,
        /// Container id on a Proxmox node (default: the running container named like Nextcloud).
        #[arg(long)]
        ctid: Option<u32>,
        /// Nextcloud install dir where it runs (default: the usual places).
        #[arg(long)]
        install_dir: Option<String>,
        /// Folder mode after a send (default 750, or 770 for a dedicated user).
        #[arg(long)]
        dir_mode: Option<String>,
        /// File mode after a send (default 640, or 660 for a dedicated user).
        #[arg(long)]
        file_mode: Option<String>,
        /// Sign in as a dedicated user instead of root (ADR 0024): on the machine Nextcloud
        /// runs on, in the web server's group, `occ` through a sudoers rule limited to it.
        /// Prints the server setup to run by hand; filebridge never changes sudoers.
        #[arg(long)]
        dedicated_user: bool,
        /// Print every command detection ran.
        #[arg(long)]
        verbose: bool,
    },
    List,
    /// Connection test (transport and integration preflight).
    Test {
        profile: String,
    },
    Remove {
        profile: String,
    },
    /// Give an SSH profile filebridge's own key (ADR 0012): generated here, added with `restrict`
    /// to the server's authorized_keys, kept in the system keyring. Undone if a step fails.
    InstallKey {
        profile: String,
        /// Don't ask first.
        #[arg(long, short)]
        yes: bool,
    },
    /// Delete filebridge's key from the server and the keyring, then check it no longer signs in.
    RemoveAccess {
        profile: String,
        /// Don't ask first.
        #[arg(long, short)]
        yes: bool,
    },
    /// Check an SSH server's host key and add it to known_hosts once you have compared its
    /// fingerprint with the server's (ADR 0015, the app's "Trust this server?"). A host with a
    /// different key on record is never changed here.
    TrustHost {
        /// SSH host (name from ~/.ssh/config is not resolved; give the address).
        #[arg(long)]
        host: String,
        #[arg(long, default_value_t = 22)]
        port: u16,
        /// Defaults to ~/.ssh/known_hosts.
        #[arg(long)]
        known_hosts: Option<PathBuf>,
        /// Don't ask: trust only if the server presents this fingerprint (`SHA256:…`).
        #[arg(long)]
        fingerprint: Option<String>,
    },
    /// Store an SFTP profile's password in the system keyring (asked without echo, or read
    /// from stdin when piped; never an argument, which shells keep in their history).
    SetPassword {
        profile: String,
        /// Remove the stored password instead.
        #[arg(long)]
        remove: bool,
    },
}

fn target(windows: bool) -> TargetSpec {
    if windows {
        TargetSpec {
            family: Family::Windows,
            case_sensitive: false,
            ..TargetSpec::default()
        }
    } else {
        TargetSpec::default()
    }
}

pub async fn run(g: &Global, cmd: ProfileCmd) -> Result<()> {
    match cmd {
        ProfileCmd::AddLocal {
            name,
            root,
            windows,
        } => {
            let app = App::open(g, Access::Exclusive, true).await?;
            let root = std::path::absolute(&root)?;
            save(
                &app,
                Profile {
                    id: String::new(),
                    name,
                    destination: Destination {
                        transport: RcloneTransport::ID.into(),
                        backend: Backend::Local,
                        root: root.to_string_lossy().into_owned(),
                    },
                    integration: None,
                    integration_settings: serde_json::Value::Null,
                    target: target(windows),
                },
            )
            .await
        }
        ProfileCmd::AddSftp {
            name,
            ssh,
            root,
            windows,
        } => {
            if !root.starts_with('/') {
                bail!("--root must be an absolute path on the server");
            }
            let app = App::open(g, Access::Exclusive, true).await?;
            save(
                &app,
                Profile {
                    id: String::new(),
                    name,
                    destination: Destination {
                        transport: RcloneTransport::ID.into(),
                        backend: ssh.backend(),
                        root,
                    },
                    integration: None,
                    integration_settings: serde_json::Value::Null,
                    target: target(windows),
                },
            )
            .await
        }
        ProfileCmd::AddNextcloudWebdav {
            name,
            server,
            user,
            folder,
        } => {
            let app = App::open(g, Access::Exclusive, true).await?;
            let app_password = read_password(&format!("App password for {user}: "))?;
            let res = app
                .ops
                .add_nextcloud_webdav(filebridge_ipc::NextcloudWebdavRequest {
                    name,
                    server,
                    user,
                    app_password: filebridge_ipc::Password::new(app_password),
                    folder,
                })
                .await
                .map_err(|e| anyhow::anyhow!("{}: {}", e.code, e.message))?;
            print_steps(&res.steps);
            let Some(saved) = res.profile else {
                bail!("the profile was not saved; fix the failed step and try again");
            };
            println!("saved profile {:?} ({})", saved.name, &saved.id[..8]);
            Ok(())
        }
        ProfileCmd::AddImmich {
            name,
            server,
            root,
            local,
            ssh,
            library,
            import_path,
        } => {
            let backend = match (local, ssh) {
                (true, _) => Backend::Local,
                (false, Some(ssh)) => ssh.backend(),
                (false, None) => bail!("give the SFTP server with --host, or use --local"),
            };
            let root = if local {
                std::path::absolute(&root)?.to_string_lossy().into_owned()
            } else if root.starts_with('/') {
                root
            } else {
                bail!("--root must be an absolute path on the server");
            };
            let app = App::open(g, Access::Exclusive, true).await?;
            let api_key = read_password("Immich API key: ")?;
            let res = app
                .ops
                .add_immich(filebridge_ipc::ImmichRequest {
                    name,
                    destination: Destination {
                        transport: RcloneTransport::ID.into(),
                        backend,
                        root,
                    },
                    password: None,
                    server,
                    api_key: filebridge_ipc::Password::new(api_key),
                    library_id: library,
                    import_path,
                })
                .await
                .map_err(|e| anyhow::anyhow!("{}: {}", e.code, e.message))?;
            print_steps(&res.steps);
            if !res.containers.is_empty() {
                println!("libraries (pass one with --library):");
                for l in &res.containers {
                    println!("  {l}");
                }
            }
            let Some(saved) = res.profile else {
                bail!("the profile was not saved; fix the failed step and try again");
            };
            println!("saved profile {:?} ({})", saved.name, &saved.id[..8]);
            Ok(())
        }
        ProfileCmd::AddNextcloud {
            name,
            ssh,
            nc_user,
            folder,
            ctid,
            install_dir,
            dir_mode,
            file_mode,
            dedicated_user,
            verbose,
        } => {
            let app = App::open(g, Access::Exclusive, true).await?;
            eprintln!("connecting to {}@{}:{} …", ssh.user, ssh.host, ssh.port);
            let req = DetectNextcloudRequest {
                name,
                host: ssh.host.clone(),
                port: ssh.port,
                user: ssh.user.clone(),
                key_file: ssh
                    .key_file
                    .as_ref()
                    .map(|p| p.to_string_lossy().into_owned()),
                known_hosts_file: ssh
                    .known_hosts
                    .as_ref()
                    .map(|p| p.to_string_lossy().into_owned()),
                nextcloud_user: nc_user,
                folder,
                ctid,
                install_dir,
                dedicated_user,
            };
            let modes = match (dir_mode, file_mode) {
                (None, None) => None,
                (d, f) => {
                    let (dd, df) = if dedicated_user {
                        (
                            NextcloudDirectProfile::DEDICATED_DIR_MODE,
                            NextcloudDirectProfile::DEDICATED_FILE_MODE,
                        )
                    } else {
                        (
                            NextcloudDirectProfile::DEFAULT_DIR_MODE,
                            NextcloudDirectProfile::DEFAULT_FILE_MODE,
                        )
                    };
                    Some((d.unwrap_or(dd.into()), f.unwrap_or(df.into())))
                }
            };
            let res = app
                .host
                .detect_nextcloud(req, modes)
                .await
                .map_err(|e| anyhow::anyhow!("{}: {}", e.code, e.message))?;
            for step in &res.steps {
                let mark = match step.status.as_str() {
                    "ok" => "✓",
                    "warn" => "!",
                    _ => "✗",
                };
                println!("  {mark} {:<28} {}", step.id, step.message);
                if verbose {
                    for c in &step.commands {
                        println!("      $ {c}");
                    }
                }
            }
            if !res.server_setup.is_empty() {
                println!(
                    "server setup for the dedicated user (read it, then run it as root yourself):"
                );
                for line in &res.server_setup {
                    println!("    {line}");
                }
            }
            let Some(saved) = res.profile else {
                if res.containers.len() > 1 {
                    println!("containers found: {}", res.containers.join(", "));
                    println!("pick one with --ctid");
                }
                bail!("detection did not finish; fix the failed step and run it again");
            };
            println!(
                "{}",
                match res.detector.as_str() {
                    "host" => "Nextcloud runs on the SSH host itself",
                    "docker" => "Nextcloud runs in a Docker container on the SSH host",
                    "snap" => "Nextcloud runs as a snap on the SSH host",
                    _ => "Nextcloud runs in a container on the Proxmox node",
                }
            );
            println!("files go to {}", saved.destination.root);
            println!("saved profile {:?} ({})", saved.name, &saved.id[..8]);
            println!("test it with: filebridge profile test {:?}", saved.name);
            Ok(())
        }
        ProfileCmd::List => {
            let app = App::open(g, Access::Read, false).await?;
            let profiles = app.ops.list_profiles().await?;
            if profiles.is_empty() {
                println!("no profiles yet; add one with `filebridge profile add-…`");
            }
            for p in profiles {
                let backend = match &p.destination.backend {
                    Backend::Local => "local".to_owned(),
                    Backend::Sftp { host, user, .. } => {
                        format!("sftp {}@{host}", user.as_deref().unwrap_or(""))
                    }
                    Backend::Webdav { url, .. } => format!("webdav {url}"),
                };
                println!(
                    "{}  {:<24} {backend}:{}{}",
                    &p.id[..8],
                    p.name,
                    p.destination.root,
                    p.integration
                        .as_deref()
                        .map(|i| format!("  [{i}]"))
                        .unwrap_or_default()
                );
            }
            Ok(())
        }
        ProfileCmd::Test { profile } => {
            let app = App::open(g, Access::Read, true).await?;
            let p = app.profile(&profile).await?;
            let test = app.ops.test_profile(p.id.clone()).await?;
            print_preflight(&test.transport);
            if let Some(i) = &test.integration {
                print_preflight(i);
            }
            let failed = test.transport.has_failures()
                || test.integration.as_ref().is_some_and(|i| i.has_failures());
            if failed {
                bail!("the connection test failed");
            }
            Ok(())
        }
        ProfileCmd::InstallKey { profile, yes } => {
            let app = App::open(g, Access::Exclusive, true).await?;
            let p = app.profile(&profile).await?;
            if !yes
                && !crate::send_cmd::confirm(&format!(
                    "Add a filebridge key (restricted) to the server's authorized_keys for {} and use it from now on?",
                    p.name
                ))?
            {
                return Ok(());
            }
            let report = app
                .ops
                .install_app_key(p.id.clone())
                .await
                .map_err(|e| anyhow::anyhow!("{}: {}", e.code, e.message))?;
            print_access(&report)
        }
        ProfileCmd::RemoveAccess { profile, yes } => {
            let app = App::open(g, Access::Exclusive, true).await?;
            let p = app.profile(&profile).await?;
            if !yes
                && !crate::send_cmd::confirm(&format!(
                    "Delete filebridge's key for {} from the server and the keyring?",
                    p.name
                ))?
            {
                return Ok(());
            }
            let report = app
                .ops
                .remove_access(p.id.clone())
                .await
                .map_err(|e| anyhow::anyhow!("{}: {}", e.code, e.message))?;
            print_access(&report)
        }
        ProfileCmd::TrustHost {
            host,
            port,
            known_hosts,
            fingerprint,
        } => trust_host(g, host, port, known_hosts, fingerprint).await,
        ProfileCmd::SetPassword { profile, remove } => {
            let app = App::open(g, Access::Exclusive, false).await?;
            let p = app.profile(&profile).await?;
            let password = if remove {
                None
            } else {
                Some(filebridge_ipc::Password::new(read_password(&format!(
                    "Password for {}: ",
                    p.name
                ))?))
            };
            app.ops.set_profile_password(p.id.clone(), password).await?;
            if remove {
                println!("password of {} removed from the keyring", p.name);
            } else {
                println!("password of {} stored in the system keyring", p.name);
            }
            Ok(())
        }
        ProfileCmd::Remove { profile } => {
            let app = App::open(g, Access::Exclusive, false).await?;
            let p = app.profile(&profile).await?;
            app.ops.delete_profile(p.id.clone()).await?;
            println!("removed {}", p.name);
            Ok(())
        }
    }
}

async fn trust_host(
    g: &Global,
    host: String,
    port: u16,
    known_hosts: Option<PathBuf>,
    expected: Option<String>,
) -> Result<()> {
    use filebridge_ipc::HostKeyStatus;
    // Read access: known_hosts is not in the data directory, so this works beside the service.
    let app = App::open(g, Access::Read, true).await?;
    let file = known_hosts.map(|p| p.to_string_lossy().into_owned());
    let scan = app
        .ops
        .scan_host_key(host.clone(), port, file.clone())
        .await
        .map_err(|e| anyhow::anyhow!("{}: {}", e.code, e.message))?;
    let key_type = scan.key_type.clone().unwrap_or_default();
    let presented = scan.fingerprint.clone().unwrap_or_default();
    match scan.status {
        HostKeyStatus::Known => {
            println!(
                "{host}:{port} is already trusted in {} ({key_type} {presented})",
                scan.known_hosts_file
            );
            return Ok(());
        }
        HostKeyStatus::Changed => bail!(
            "{} line {} holds another key for {host}:{port}, and it now presents {}: the server \
             changed or the connection is being intercepted. If whoever runs it confirms the \
             change, remove the old line (`ssh-keygen -R {}`) and try again",
            scan.known_hosts_file,
            scan.line.map_or("?".into(), |l| l.to_string()),
            if presented.is_empty() {
                "none of the key types on record".to_owned()
            } else {
                format!("{key_type} {presented}")
            },
            if port == 22 {
                host.clone()
            } else {
                format!("'[{host}]:{port}'")
            },
        ),
        HostKeyStatus::Unknown => {}
    }
    println!("{host}:{port} presents {key_type} {presented}");
    match &expected {
        Some(fp) if *fp != presented => {
            bail!("the server presents {presented}, not {fp}; nothing was added")
        }
        Some(_) => {}
        None => {
            println!(
                "Compare it with what the server shows for its own key: \
                 `ssh-keygen -lf /etc/ssh/ssh_host_{}_key.pub`",
                host_key_file_type(&key_type)
            );
            if !std::io::IsTerminal::is_terminal(&std::io::stdin()) {
                bail!("not a terminal; pass --fingerprint {presented} to trust it without asking");
            }
            if !crate::send_cmd::confirm(&format!(
                "Trust this key and add it to {}?",
                scan.known_hosts_file
            ))? {
                println!("nothing was added");
                return Ok(());
            }
        }
    }
    let trusted = app
        .ops
        .trust_host_key(host.clone(), port, file, presented)
        .await
        .map_err(|e| anyhow::anyhow!("{}: {}", e.code, e.message))?;
    println!("{host}:{port} added to {}", trusted.known_hosts_file);
    Ok(())
}

/// The `<type>` in `/etc/ssh/ssh_host_<type>_key.pub` for an SSH key type name.
fn host_key_file_type(key_type: &str) -> &str {
    match key_type {
        "ssh-ed25519" => "ed25519",
        t if t.starts_with("ecdsa-") => "ecdsa",
        "ssh-rsa" | "rsa-sha2-256" | "rsa-sha2-512" => "rsa",
        _ => "<type>",
    }
}

fn print_access(report: &filebridge_ipc::AccessReport) -> Result<()> {
    for step in &report.steps {
        let mark = match step.status.as_str() {
            "ok" => "✓",
            "warn" => "!",
            _ => "✗",
        };
        println!("  {mark} {:<9} {}", step.id, step.message);
    }
    if report.profile.is_none() || report.steps.iter().any(|s| s.status == "fail") {
        bail!("not finished; see the step that failed");
    }
    Ok(())
}

/// A password without echo on a terminal; the first line of stdin when piped.
/// A wizard's steps, one line each: ✓, ! or ✗, the step and what it found.
fn print_steps(steps: &[filebridge_ipc::DetectStepView]) {
    for step in steps {
        let mark = match step.status.as_str() {
            "ok" => "✓",
            "warn" => "!",
            _ => "✗",
        };
        println!("  {mark} {:<11} {}", step.id, step.message);
    }
}

fn read_password(prompt: &str) -> Result<String> {
    use std::io::{BufRead, IsTerminal};
    let value = if std::io::stdin().is_terminal() {
        rpassword::prompt_password(prompt)?
    } else {
        let mut line = String::new();
        std::io::stdin().lock().read_line(&mut line)?;
        line.trim_end_matches(['\n', '\r']).to_owned()
    };
    if value.is_empty() {
        anyhow::bail!("an empty password; use --remove to delete the stored one");
    }
    Ok(value)
}

async fn save(app: &App, profile: Profile) -> Result<()> {
    let saved = app.ops.save_profile(profile).await?;
    println!("saved profile {:?} ({})", saved.name, &saved.id[..8]);
    println!("test it with: filebridge profile test {:?}", saved.name);
    Ok(())
}

pub fn print_preflight(p: &PreflightView) {
    let free = p
        .free_bytes
        .map(|b| format!(" ({} free)", crate::send_cmd::human_bytes(b)))
        .unwrap_or_default();
    println!("{}{free}", p.source);
    for c in &p.checks {
        let mark = match c.status.as_str() {
            "ok" => "✓",
            "warn" => "!",
            _ => "✗",
        };
        println!("  {mark} {:<28} {}", c.id, c.message);
    }
}
