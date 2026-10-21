//! `filebridge profile …`: destinations.

use std::path::PathBuf;

use anyhow::{Result, bail};
use clap::Subcommand;
use filebridge_core::{Family, PreflightView, Profile, TargetSpec};
use filebridge_integration_nextcloud::NextcloudDirectProfile;
use filebridge_ipc::DetectNextcloudRequest;
use filebridge_transport_api::{Backend, Destination};

use crate::app::{App, Global};
use filebridge_host::Access;

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
    /// Nextcloud in a Proxmox LXC, direct mode: detects the container, data folder, `occ` and
    /// owner over SSH, then saves the profile. Runs read-only commands on the host.
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
        /// Container id (default: the running container named like Nextcloud).
        #[arg(long)]
        ctid: Option<u32>,
        /// Nextcloud install dir inside the container (default: the usual places).
        #[arg(long)]
        install_dir: Option<String>,
        #[arg(long, default_value = NextcloudDirectProfile::DEFAULT_DIR_MODE)]
        dir_mode: String,
        #[arg(long, default_value = NextcloudDirectProfile::DEFAULT_FILE_MODE)]
        file_mode: String,
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
}

fn target(windows: bool) -> TargetSpec {
    if windows {
        TargetSpec {
            family: Family::Windows,
            case_sensitive: false,
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
                        transport: "rclone".into(),
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
                        transport: "rclone".into(),
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
        ProfileCmd::AddNextcloud {
            name,
            ssh,
            nc_user,
            folder,
            ctid,
            install_dir,
            dir_mode,
            file_mode,
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
            };
            let res =
                filebridge_host::detect_nextcloud(&app.engine, req, Some((dir_mode, file_mode)))
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
            let Some(saved) = res.profile else {
                if res.containers.len() > 1 {
                    println!("containers found: {}", res.containers.join(", "));
                    println!("pick one with --ctid");
                }
                bail!("detection did not finish; fix the failed step and run it again");
            };
            println!("files go to {}", saved.destination.root);
            println!("saved profile {:?} ({})", saved.name, &saved.id[..8]);
            println!("test it with: filebridge profile test {:?}", saved.name);
            Ok(())
        }
        ProfileCmd::List => {
            let app = App::open(g, Access::Read, false).await?;
            let profiles = app.engine.list_profiles().await?;
            if profiles.is_empty() {
                println!("no profiles yet; add one with `filebridge profile add-…`");
            }
            for p in profiles {
                let backend = match &p.destination.backend {
                    Backend::Local => "local".to_owned(),
                    Backend::Sftp { host, user, .. } => {
                        format!("sftp {}@{host}", user.as_deref().unwrap_or(""))
                    }
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
            let test = app.engine.test_profile(&p.id).await?;
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
        ProfileCmd::Remove { profile } => {
            let app = App::open(g, Access::Exclusive, false).await?;
            let p = app.profile(&profile).await?;
            app.engine.delete_profile(&p.id).await?;
            println!("removed {}", p.name);
            Ok(())
        }
    }
}

async fn save(app: &App, profile: Profile) -> Result<()> {
    let saved = app.engine.save_profile(profile).await?;
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
