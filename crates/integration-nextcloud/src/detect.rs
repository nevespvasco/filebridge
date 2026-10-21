//! Profile detection for direct mode. Detection is a list of [`Detector`]s so a `DockerDetector`
//! can be added without changing the wizard; [`LxcDetector`] covers Nextcloud in a Proxmox LXC.
//!
//! Every step reports the commands it ran (with exit status and stderr) for the wizard's
//! "Details" panel. Stdout is never kept: `config.php` holds the database password, which is
//! also why only the `datadirectory` line is read from it (`grep`, not `cat`).

use async_trait::async_trait;
use filebridge_integration_api::{Check, CheckStatus};
use serde::{Deserialize, Serialize};

use crate::parse::{self, LxcConfig, LxcContainer, MountSource};
use crate::profile::{Exec, NextcloudDirectProfile, Owner, RunAs, Runtime, path_join};
use crate::shell::{Output, RemoteShell, SshTarget};

/// What the wizard knows before detection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DetectInput {
    pub ssh: SshTarget,
    /// Nextcloud user id whose `files/` receives sends (from the WebDAV step).
    pub nextcloud_user: String,
    /// Container to use; when absent, the one running container named like Nextcloud.
    pub ctid: Option<u32>,
    /// Nextcloud install dir inside the container; when absent, the usual places are tried.
    pub install_dir: Option<String>,
}

/// A command a step ran, for the Details panel.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RanCommand {
    pub command: String,
    pub status: i32,
    pub stderr: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DetectStep {
    pub check: Check,
    pub commands: Vec<RanCommand>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Detection {
    pub detector: String,
    /// In order; detection stops at the first failed step.
    pub steps: Vec<DetectStep>,
    /// Containers seen, so the wizard can let the user pick when the choice was ambiguous.
    pub containers: Vec<LxcContainer>,
    /// Present when every step passed (warnings allowed); ready to save.
    pub profile: Option<NextcloudDirectProfile>,
}

#[async_trait]
pub trait Detector: Send + Sync {
    fn id(&self) -> &str;
    async fn detect(&self, shell: &dyn RemoteShell, input: &DetectInput) -> Detection;
}

/// Places `occ` usually lives in an LXC install.
pub const INSTALL_DIRS: &[&str] = &[
    "/var/www/nextcloud",
    "/var/www/html/nextcloud",
    "/var/www/html",
    "/usr/share/webapps/nextcloud",
    "/srv/nextcloud",
    "/opt/nextcloud",
];

/// Fallback web-server user when the owner of `config.php` can't be read.
pub const DEFAULT_WEB_USER: &str = "www-data";

/// Proxmox host over SSH → `pct` → Nextcloud container.
#[derive(Debug, Default, Clone)]
pub struct LxcDetector;

impl LxcDetector {
    pub const ID: &'static str = "proxmox-lxc";
}

/// Records commands and step results.
struct Recorder<'a> {
    shell: &'a dyn RemoteShell,
    steps: Vec<DetectStep>,
    pending: Vec<RanCommand>,
}

/// A failed step: message for the user. The step is recorded before returning it.
struct Failed;

impl<'a> Recorder<'a> {
    async fn run(&mut self, cmd: String) -> Result<Output, Failed> {
        match self.shell.run(&cmd).await {
            Ok(out) => {
                self.pending.push(RanCommand {
                    command: cmd,
                    status: out.status,
                    stderr: out.stderr.trim().to_string(),
                });
                Ok(out)
            }
            Err(e) => {
                self.pending.push(RanCommand {
                    command: cmd,
                    status: -1,
                    stderr: e.message.clone(),
                });
                Err(Failed)
            }
        }
    }

    fn step(&mut self, id: &str, status: CheckStatus, message: impl Into<String>) {
        self.steps.push(DetectStep {
            check: Check {
                id: id.to_string(),
                status,
                message: message.into(),
            },
            commands: std::mem::take(&mut self.pending),
        });
    }

    fn fail(&mut self, id: &str, message: impl Into<String>) -> Failed {
        self.step(id, CheckStatus::Fail, message);
        Failed
    }

    /// Runs `cmd`; a lost connection fails step `id`.
    async fn run_in(&mut self, id: &str, cmd: String) -> Result<Output, Failed> {
        match self.run(cmd).await {
            Ok(out) => Ok(out),
            Err(Failed) => {
                let why = self
                    .pending
                    .last()
                    .map(|c| c.stderr.clone())
                    .unwrap_or_default();
                Err(self.fail(id, format!("could not run the command: {why}")))
            }
        }
    }
}

fn looks_like_nextcloud(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    n.contains("nextcloud") || n == "nc" || n.starts_with("nc-")
}

#[async_trait]
impl Detector for LxcDetector {
    fn id(&self) -> &str {
        Self::ID
    }

    async fn detect(&self, shell: &dyn RemoteShell, input: &DetectInput) -> Detection {
        let mut rec = Recorder {
            shell,
            steps: Vec::new(),
            pending: Vec::new(),
        };
        let mut containers = Vec::new();
        let profile = detect_lxc(&mut rec, input, &mut containers).await.ok();
        Detection {
            detector: Self::ID.to_string(),
            steps: rec.steps,
            containers,
            profile,
        }
    }
}

async fn detect_lxc(
    rec: &mut Recorder<'_>,
    input: &DetectInput,
    containers: &mut Vec<LxcContainer>,
) -> Result<NextcloudDirectProfile, Failed> {
    // 1. `pct list`, as the SSH user or through `sudo -n`.
    let plain = Exec {
        host_sudo: false,
        runtime: Runtime::ProxmoxLxc { ctid: 0 },
    };
    let mut out = rec.run_in("pct-list", plain.host(&["pct", "list"])).await?;
    let mut host_sudo = false;
    if out.status == 127 {
        return Err(rec.fail(
            "pct-list",
            "`pct` not found: the SSH host must be the Proxmox node that runs the container",
        ));
    }
    if !out.success() {
        let sudo = Exec {
            host_sudo: true,
            ..plain.clone()
        };
        out = rec.run_in("pct-list", sudo.host(&["pct", "list"])).await?;
        if !out.success() {
            return Err(rec.fail(
                "pct-list",
                format!(
                    "`pct list` needs root: log in as root or allow `sudo pct` for {} ({})",
                    input.ssh.user,
                    out.short_error()
                ),
            ));
        }
        host_sudo = true;
    }
    *containers = parse::pct_list(&out.stdout);
    let chosen = match input.ctid {
        Some(id) => containers.iter().find(|c| c.ctid == id).cloned(),
        None => {
            let matches: Vec<&LxcContainer> = containers
                .iter()
                .filter(|c| c.running() && looks_like_nextcloud(&c.name))
                .collect();
            match matches[..] {
                [one] => Some(one.clone()),
                _ => {
                    return Err(rec.fail(
                        "pct-list",
                        format!(
                            "found {} running containers named like Nextcloud; choose one",
                            matches.len()
                        ),
                    ));
                }
            }
        }
    };
    let Some(ct) = chosen else {
        return Err(rec.fail("pct-list", format!("container {:?} not found", input.ctid)));
    };
    if !ct.running() {
        return Err(rec.fail(
            "pct-list",
            format!(
                "container {} ({}) is {}; start it first",
                ct.ctid, ct.name, ct.status
            ),
        ));
    }
    rec.step(
        "pct-list",
        CheckStatus::Ok,
        format!("container {} ({}) is running", ct.ctid, ct.name),
    );
    let exec = Exec {
        host_sudo,
        runtime: Runtime::ProxmoxLxc { ctid: ct.ctid },
    };

    // 2. `pct config`: bind mounts and idmap.
    let ctid = ct.ctid.to_string();
    let out = rec
        .run_in("pct-config", exec.host(&["pct", "config", &ctid]))
        .await?;
    if !out.success() {
        return Err(rec.fail(
            "pct-config",
            format!("`pct config` failed: {}", out.short_error()),
        ));
    }
    let cfg = parse::pct_config(&out.stdout);
    rec.step(
        "pct-config",
        CheckStatus::Ok,
        format!(
            "{} mount point(s), {}, {} idmap",
            cfg.mounts.len(),
            if cfg.unprivileged {
                "unprivileged"
            } else {
                "privileged"
            },
            if cfg.idmap.is_empty() {
                "default"
            } else {
                "custom"
            },
        ),
    );

    // 3. `datadirectory` from config.php, mapped to the host through a bind mount.
    let install_dir = match &input.install_dir {
        Some(d) => d.trim_end_matches('/').to_string(),
        None => {
            let mut argv = vec!["ls".to_string(), "-d".to_string()];
            argv.extend(
                INSTALL_DIRS
                    .iter()
                    .map(|d| format!("{d}/config/config.php")),
            );
            // `ls` fails for the missing candidates but lists the ones that exist.
            let out = rec.run_in("datadirectory", exec.container(&argv)).await?;
            let found = out.stdout.lines().map(str::trim).find(|l| !l.is_empty());
            match found.and_then(|l| l.strip_suffix("/config/config.php")) {
                Some(d) => d.to_string(),
                None => {
                    return Err(rec.fail(
                        "datadirectory",
                        format!(
                            "no config/config.php in {}; set the install directory by hand",
                            INSTALL_DIRS.join(", ")
                        ),
                    ));
                }
            }
        }
    };
    let config_php = path_join(&install_dir, "config/config.php");
    let out = rec
        .run_in(
            "datadirectory",
            exec.container(&["grep", "-F", "datadirectory", &config_php]),
        )
        .await?;
    let Some(data_dir) = parse::datadirectory(&out.stdout) else {
        return Err(rec.fail(
            "datadirectory",
            format!("no 'datadirectory' in {config_php}: Nextcloud uses its default data folder"),
        ));
    };
    let host_data_dir = match host_path(rec, &exec, &cfg, &data_dir).await? {
        Some(p) => p,
        None => {
            return Err(rec.fail(
                "datadirectory",
                format!(
                    "Nextcloud keeps its data in {data_dir}, on the container's own disk (no \
                     bind mount covers it); direct mode writes through a bind mount, so files \
                     would land where Nextcloud doesn't look"
                ),
            ));
        }
    };
    rec.step(
        "datadirectory",
        CheckStatus::Ok,
        format!("{data_dir} in the container is {host_data_dir} on the host"),
    );

    // 3b. Both paths exist, and so does the user's files folder.
    let user = &input.nextcloud_user;
    let host_files = path_join(&path_join(&host_data_dir, user), "files");
    for (cmd, what) in [
        (
            exec.container(&["test", "-d", &data_dir]),
            format!("{data_dir} (container)"),
        ),
        (
            exec.host(&["test", "-d", &host_data_dir]),
            format!("{host_data_dir} (host)"),
        ),
        (
            exec.host(&["test", "-d", &host_files]),
            format!("{host_files} (host)"),
        ),
    ] {
        let out = rec.run_in("data-path", cmd).await?;
        if !out.success() {
            return Err(rec.fail("data-path", format!("{what} does not exist")));
        }
    }
    rec.step("data-path", CheckStatus::Ok, format!("{host_files} exists"));

    // 4. `occ` and the web-server user (owner of config.php).
    let occ = path_join(&install_dir, "occ");
    let out = rec
        .run_in("occ", exec.container(&["test", "-f", &occ]))
        .await?;
    if !out.success() {
        return Err(rec.fail("occ", format!("{occ} not found")));
    }
    let out = rec
        .run_in("occ", exec.container(&["stat", "-c", "%U", &config_php]))
        .await?;
    let owner_name = out.stdout.trim();
    let web_user = if out.success() && !owner_name.is_empty() && owner_name != "root" {
        owner_name.to_string()
    } else {
        DEFAULT_WEB_USER.to_string()
    };
    rec.step("occ", CheckStatus::Ok, format!("{occ}, run as {web_user}"));

    // 5. sudo in the container, or `su -s /bin/bash`; proven by `occ status`.
    let out = rec
        .run_in("run-as", exec.container(&["sh", "-c", "command -v sudo"]))
        .await?;
    let run_as = if out.success() {
        RunAs::Sudo
    } else {
        RunAs::Su
    };
    let php = "php";
    let out = rec
        .run_in(
            "run-as",
            exec.occ(run_as, &web_user, php, &occ, &["status", "--output=json"]),
        )
        .await?;
    match parse::occ_status(&out.stdout) {
        Some(s) if out.success() && s.installed => rec.step(
            "run-as",
            if s.maintenance {
                CheckStatus::Warn
            } else {
                CheckStatus::Ok
            },
            format!(
                "Nextcloud {} answers through {}{}",
                s.versionstring,
                match run_as {
                    RunAs::Sudo => "sudo",
                    RunAs::Su => "su (no sudo in the container)",
                },
                if s.maintenance {
                    "; maintenance mode is on"
                } else {
                    ""
                }
            ),
        ),
        _ => {
            return Err(rec.fail(
                "run-as",
                format!("`occ status` did not work: {}", out.short_error()),
            ));
        }
    }

    // 6. Owner on the host: the web user's ids through the idmap.
    let out = rec
        .run_in("owner", exec.container(&["id", &web_user]))
        .await?;
    let Some((uid, gid)) = parse::id_output(&out.stdout) else {
        return Err(rec.fail("owner", format!("no user {web_user} in the container")));
    };
    let (Some(huid), Some(hgid)) = (cfg.map_id('u', uid), cfg.map_id('g', gid)) else {
        return Err(rec.fail(
            "owner",
            format!("{web_user} ({uid}:{gid}) is not mapped to the host by the container's idmap"),
        ));
    };
    let owner = Owner {
        uid: huid,
        gid: hgid,
    };
    let out = rec
        .run_in("owner", exec.host(&["stat", "-c", "%u:%g", &host_data_dir]))
        .await?;
    match parse::uid_gid(&out.stdout) {
        Some(found) if found == (huid, hgid) => rec.step(
            "owner",
            CheckStatus::Ok,
            format!(
                "files will be owned by {} ({web_user} {uid}:{gid} in the container)",
                owner.chown_spec()
            ),
        ),
        found => rec.step(
            "owner",
            CheckStatus::Warn,
            format!(
                "files will be owned by {}, but {host_data_dir} is owned by {}",
                owner.chown_spec(),
                found.map_or("an unknown owner".into(), |(u, g)| format!("{u}:{g}"))
            ),
        ),
    }

    Ok(NextcloudDirectProfile {
        ssh: input.ssh.clone(),
        host_sudo,
        runtime: exec.runtime,
        install_dir,
        php: php.to_string(),
        run_as,
        web_user,
        data_dir,
        host_data_dir,
        user: user.clone(),
        owner,
        dir_mode: NextcloudDirectProfile::DEFAULT_DIR_MODE.into(),
        file_mode: NextcloudDirectProfile::DEFAULT_FILE_MODE.into(),
    })
}

/// Host path of container path `path`, through the mount that covers it. `None` when no mount
/// does (the data is on the container's root disk).
async fn host_path(
    rec: &mut Recorder<'_>,
    exec: &Exec,
    cfg: &LxcConfig,
    path: &str,
) -> Result<Option<String>, Failed> {
    let Some((mount, rest)) = cfg.mount_for(path) else {
        return Ok(None);
    };
    let base = match &mount.source {
        MountSource::HostPath(p) => p.clone(),
        MountSource::Volume(vol) => {
            let out = rec
                .run_in("datadirectory", exec.host(&["pvesm", "path", vol]))
                .await?;
            if !out.success() || out.stdout.trim().is_empty() {
                return Err(rec.fail(
                    "datadirectory",
                    format!("could not resolve volume {vol}: {}", out.short_error()),
                ));
            }
            out.stdout.trim().to_string()
        }
    };
    Ok(Some(path_join(&base, &rest)))
}
