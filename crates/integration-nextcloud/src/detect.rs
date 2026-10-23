//! Profile detection for direct mode. Detection is a list of [`Detector`]s so a `DockerDetector`
//! can be added without changing the wizard: [`LxcDetector`] covers Nextcloud in a Proxmox LXC
//! (the SSH host is the node), [`HostDetector`] Nextcloud on the SSH host itself (its LXC or VM
//! reached directly, or a bare-metal server). The wizard tries them in order until one applies.
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
    /// Container to use; when absent, the one running container named like Nextcloud (or, on a
    /// host without `pct`, Nextcloud on the host itself).
    pub ctid: Option<u32>,
    /// Nextcloud install dir where it runs; when absent, the usual places are tried.
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
    /// The detector that produced the last steps: [`LxcDetector::ID`] or [`HostDetector::ID`].
    pub detector: String,
    /// False when this detector's setup is not on the SSH host (no `pct` there), so the wizard
    /// goes on to the next detector; its steps stay in front of that one's.
    pub applies: bool,
    /// In order; detection stops at the first failed step.
    pub steps: Vec<DetectStep>,
    /// Containers seen, so the wizard can let the user pick when the choice was ambiguous.
    pub containers: Vec<LxcContainer>,
    /// Present when every step passed (warnings allowed); ready to save.
    pub profile: Option<NextcloudDirectProfile>,
    /// The failed step found Nextcloud but not the rights direct mode needs (root or `sudo -n`,
    /// writing the files folder), rather than no usable Nextcloud.
    #[serde(default)]
    pub lacks_rights: bool,
}

#[async_trait]
pub trait Detector: Send + Sync {
    fn id(&self) -> &str;
    async fn detect(&self, shell: &dyn RemoteShell, input: &DetectInput) -> Detection;
}

/// Places `occ` usually lives in a package or tarball install.
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

/// Nextcloud on the SSH host itself: commands run there directly, no idmap.
#[derive(Debug, Default, Clone)]
pub struct HostDetector;

impl HostDetector {
    pub const ID: &'static str = "host";
}

/// The Proxmox CLI, for a non-root SSH user whose `PATH` has no `/usr/sbin`.
const PCT_PATH: &str = "/usr/sbin/pct";

/// Records commands and step results.
struct Recorder<'a> {
    shell: &'a dyn RemoteShell,
    steps: Vec<DetectStep>,
    pending: Vec<RanCommand>,
    lacks_rights: bool,
}

/// A failed step: message for the user. The step is recorded before returning it.
struct Failed;

impl<'a> Recorder<'a> {
    fn new(shell: &'a dyn RemoteShell) -> Self {
        Self {
            shell,
            steps: Vec::new(),
            pending: Vec::new(),
            lacks_rights: false,
        }
    }

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

    /// Fails step `id` for want of rights (see [`Detection::lacks_rights`]).
    fn fail_rights(&mut self, id: &str, message: impl Into<String>) -> Failed {
        self.lacks_rights = true;
        self.fail(id, message)
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
        let mut rec = Recorder::new(shell);
        let mut containers = Vec::new();
        let (applies, profile) = match detect_lxc(&mut rec, input, &mut containers).await {
            Ok(Some(p)) => (true, Some(p)),
            Ok(None) => (false, None),
            Err(Failed) => (true, None),
        };
        Detection {
            detector: Self::ID.to_string(),
            applies,
            lacks_rights: rec.lacks_rights,
            steps: rec.steps,
            containers,
            profile,
        }
    }
}

#[async_trait]
impl Detector for HostDetector {
    fn id(&self) -> &str {
        Self::ID
    }

    async fn detect(&self, shell: &dyn RemoteShell, input: &DetectInput) -> Detection {
        let mut rec = Recorder::new(shell);
        let profile = detect_host(&mut rec, input).await.ok();
        Detection {
            detector: Self::ID.to_string(),
            applies: true,
            lacks_rights: rec.lacks_rights,
            steps: rec.steps,
            containers: Vec::new(),
            profile,
        }
    }
}

/// `Ok(None)`: no `pct` on the SSH host, so Nextcloud isn't in one of its containers.
async fn detect_lxc(
    rec: &mut Recorder<'_>,
    input: &DetectInput,
    containers: &mut Vec<LxcContainer>,
) -> Result<Option<NextcloudDirectProfile>, Failed> {
    // 1. `pct list`, as the SSH user or through `sudo -n`.
    let plain = Exec {
        host_sudo: false,
        runtime: Runtime::ProxmoxLxc { ctid: 0 },
    };
    let mut out = rec.run_in("pct-list", plain.host(&["pct", "list"])).await?;
    let mut host_sudo = false;
    if out.status == 127 {
        // Not on `PATH`: either not a Proxmox node, or a non-root user without `/usr/sbin`.
        let installed = rec
            .run_in("pct-list", plain.host(&["test", "-x", PCT_PATH]))
            .await?;
        if !installed.success() {
            if input.ctid.is_some() {
                return Err(rec.fail(
                    "pct-list",
                    "`pct` not found, so this is not a Proxmox node: connect to the node that \
                     runs the container, or clear the container to use Nextcloud on this machine",
                ));
            }
            rec.step(
                "pct-list",
                CheckStatus::Ok,
                "no `pct` here, so this is not a Proxmox node: looking for Nextcloud on this \
                 machine itself",
            );
            return Ok(None);
        }
    }
    if !out.success() {
        let sudo = Exec {
            host_sudo: true,
            ..plain.clone()
        };
        out = rec.run_in("pct-list", sudo.host(&["pct", "list"])).await?;
        if !out.success() {
            return Err(rec.fail_rights(
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
    let install_dir = find_install_dir(rec, &exec, input).await?;
    let config_php = path_join(&install_dir, "config/config.php");
    let Some(data_dir) = read_datadirectory(rec, &exec, &config_php).await? else {
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
    check_dirs(
        rec,
        [
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
        ],
        &host_files,
    )
    .await?;
    check_writable(rec, &exec, &input.ssh.user, &host_files).await?;

    // 4–5. `occ`, the web-server user, and how to become it.
    let (occ, web_user) = find_occ(rec, &exec, &install_dir).await?;
    let php = "php";
    let run_as = find_run_as(rec, &exec, &web_user, php, &occ).await?;

    // 6. Owner on the host: the web user's ids through the idmap.
    let (uid, gid) = web_user_ids(rec, &exec, &web_user).await?;
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
    check_owner(
        rec,
        &exec,
        owner,
        &host_data_dir,
        &format!("{web_user} {uid}:{gid} in the container"),
    )
    .await?;

    Ok(Some(NextcloudDirectProfile {
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
    }))
}

async fn detect_host(
    rec: &mut Recorder<'_>,
    input: &DetectInput,
) -> Result<NextcloudDirectProfile, Failed> {
    // 1. Root, or `sudo -n`: setting the owner of sent files needs it, and so does `su`.
    let plain = Exec {
        host_sudo: false,
        runtime: Runtime::Host,
    };
    let out = rec.run_in("host", plain.host(&["id", "-u"])).await?;
    let host_sudo = if out.success() && out.stdout.trim() == "0" {
        false
    } else {
        let out = rec
            .run_in("host", plain.host(&["sudo", "-n", "true"]))
            .await?;
        if !out.success() {
            return Err(rec.fail_rights(
                "host",
                format!(
                    "direct mode sets the owner of the files it sends, which needs root: log in \
                     as root or allow `sudo -n` for {} ({})",
                    input.ssh.user,
                    out.short_error()
                ),
            ));
        }
        true
    };
    rec.step(
        "host",
        CheckStatus::Ok,
        format!(
            "not in a Proxmox container; commands run {}",
            if host_sudo {
                "through sudo -n"
            } else {
                "as root"
            }
        ),
    );
    let exec = Exec {
        host_sudo,
        runtime: Runtime::Host,
    };

    // 2. `datadirectory` from config.php; the SSH host sees that same path.
    let install_dir = find_install_dir(rec, &exec, input).await?;
    let config_php = path_join(&install_dir, "config/config.php");
    let (data_dir, default) = match read_datadirectory(rec, &exec, &config_php).await? {
        Some(d) => (d, false),
        // Nextcloud's own default when config.php doesn't set it.
        None => (path_join(&install_dir, "data"), true),
    };
    rec.step(
        "datadirectory",
        CheckStatus::Ok,
        if default {
            format!("{data_dir} (Nextcloud's default: config.php doesn't set datadirectory)")
        } else {
            data_dir.clone()
        },
    );

    // 2b. The data folder and the user's files folder exist.
    let user = &input.nextcloud_user;
    let files = path_join(&path_join(&data_dir, user), "files");
    check_dirs(
        rec,
        [
            (exec.host(&["test", "-d", &data_dir]), data_dir.clone()),
            (exec.host(&["test", "-d", &files]), files.clone()),
        ],
        &files,
    )
    .await?;
    check_writable(rec, &exec, &input.ssh.user, &files).await?;

    // 3–4. `occ`, the web-server user, and how to become it.
    let (occ, web_user) = find_occ(rec, &exec, &install_dir).await?;
    let php = "php";
    let run_as = find_run_as(rec, &exec, &web_user, php, &occ).await?;

    // 5. Owner: the web user's own ids (no idmap on the machine Nextcloud runs on).
    let (uid, gid) = web_user_ids(rec, &exec, &web_user).await?;
    let owner = Owner { uid, gid };
    check_owner(rec, &exec, owner, &data_dir, &web_user).await?;

    Ok(NextcloudDirectProfile {
        ssh: input.ssh.clone(),
        host_sudo,
        runtime: Runtime::Host,
        install_dir,
        php: php.to_string(),
        run_as,
        web_user,
        host_data_dir: data_dir.clone(),
        data_dir,
        user: user.clone(),
        owner,
        dir_mode: NextcloudDirectProfile::DEFAULT_DIR_MODE.into(),
        file_mode: NextcloudDirectProfile::DEFAULT_FILE_MODE.into(),
    })
}

/// Where Nextcloud runs, for messages.
fn place(exec: &Exec) -> &'static str {
    match exec.runtime {
        Runtime::ProxmoxLxc { .. } => "in the container",
        Runtime::Host => "on this machine",
    }
}

/// `DetectInput::install_dir`, or the first of [`INSTALL_DIRS`] holding `config/config.php`.
async fn find_install_dir(
    rec: &mut Recorder<'_>,
    exec: &Exec,
    input: &DetectInput,
) -> Result<String, Failed> {
    if let Some(d) = &input.install_dir {
        return Ok(d.trim_end_matches('/').to_string());
    }
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
        Some(d) => Ok(d.to_string()),
        None => Err(rec.fail(
            "datadirectory",
            format!(
                "no config/config.php {} in {}; set the install directory by hand",
                place(exec),
                INSTALL_DIRS.join(", ")
            ),
        )),
    }
}

/// The `datadirectory` line of `config_php` (only that line: the file holds the DB password).
async fn read_datadirectory(
    rec: &mut Recorder<'_>,
    exec: &Exec,
    config_php: &str,
) -> Result<Option<String>, Failed> {
    let out = rec
        .run_in(
            "datadirectory",
            exec.container(&["grep", "-F", "datadirectory", config_php]),
        )
        .await?;
    Ok(parse::datadirectory(&out.stdout))
}

/// Step `data-path`: every `(test -d command, what)` passes; `files` is named in the message.
async fn check_dirs<const N: usize>(
    rec: &mut Recorder<'_>,
    checks: [(String, String); N],
    files: &str,
) -> Result<(), Failed> {
    for (cmd, what) in checks {
        let out = rec.run_in("data-path", cmd).await?;
        if !out.success() {
            return Err(rec.fail("data-path", format!("{what} does not exist")));
        }
    }
    rec.step("data-path", CheckStatus::Ok, format!("{files} exists"));
    Ok(())
}

/// Step `occ`: `occ` exists; the web-server user is the owner of `config.php`.
/// The SSH user itself (never through sudo) can write to the user's files folder on the host:
/// SFTP writes the sent files as that user, so `sudo -n` alone is not enough.
async fn check_writable(
    rec: &mut Recorder<'_>,
    exec: &Exec,
    ssh_user: &str,
    files: &str,
) -> Result<(), Failed> {
    let plain = Exec {
        host_sudo: false,
        ..exec.clone()
    };
    let out = rec
        .run_in("writable", plain.host(&["test", "-w", files]))
        .await?;
    if !out.success() {
        return Err(rec.fail_rights(
            "writable",
            format!(
                "{ssh_user} cannot write to {files}: files are sent over SFTP as the SSH user \
                 (sudo only sets their owner and runs occ), so log in as root"
            ),
        ));
    }
    rec.step(
        "writable",
        CheckStatus::Ok,
        format!("{ssh_user} can write to {files}"),
    );
    Ok(())
}

async fn find_occ(
    rec: &mut Recorder<'_>,
    exec: &Exec,
    install_dir: &str,
) -> Result<(String, String), Failed> {
    let occ = path_join(install_dir, "occ");
    let config_php = path_join(install_dir, "config/config.php");
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
    Ok((occ, web_user))
}

/// Step `run-as`: sudo where Nextcloud runs, or `su -s /bin/bash`; proven by `occ status`.
async fn find_run_as(
    rec: &mut Recorder<'_>,
    exec: &Exec,
    web_user: &str,
    php: &str,
    occ: &str,
) -> Result<RunAs, Failed> {
    let out = rec
        .run_in("run-as", exec.container(&["sh", "-c", "command -v sudo"]))
        .await?;
    let run_as = if out.success() {
        RunAs::Sudo
    } else {
        RunAs::Su
    };
    let out = rec
        .run_in(
            "run-as",
            exec.occ(
                run_as,
                web_user,
                php,
                occ,
                &["status", "--output=json"],
                false,
            ),
        )
        .await?;
    match parse::occ_status(&out.stdout) {
        Some(s) if out.success() && s.installed => {
            rec.step(
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
                        RunAs::Sudo => "sudo".to_string(),
                        RunAs::Su => format!("su (no sudo {})", place(exec)),
                    },
                    if s.maintenance {
                        "; maintenance mode is on"
                    } else {
                        ""
                    }
                ),
            );
            Ok(run_as)
        }
        _ => Err(rec.fail(
            "run-as",
            format!("`occ status` did not work: {}", out.short_error()),
        )),
    }
}

/// `id <web_user>` where Nextcloud runs; a missing user fails step `owner`.
async fn web_user_ids(
    rec: &mut Recorder<'_>,
    exec: &Exec,
    web_user: &str,
) -> Result<(u32, u32), Failed> {
    let out = rec
        .run_in("owner", exec.container(&["id", web_user]))
        .await?;
    match parse::id_output(&out.stdout) {
        Some(ids) => Ok(ids),
        None => Err(rec.fail("owner", format!("no user {web_user} {}", place(exec)))),
    }
}

/// Step `owner`: `owner` cross-checked against the owner of the data folder on the SSH host
/// (a mismatch is a warning). `who` says whose ids these are.
async fn check_owner(
    rec: &mut Recorder<'_>,
    exec: &Exec,
    owner: Owner,
    host_data_dir: &str,
    who: &str,
) -> Result<(), Failed> {
    let out = rec
        .run_in("owner", exec.host(&["stat", "-c", "%u:%g", host_data_dir]))
        .await?;
    match parse::uid_gid(&out.stdout) {
        Some(found) if found == (owner.uid, owner.gid) => rec.step(
            "owner",
            CheckStatus::Ok,
            format!("files will be owned by {} ({who})", owner.chown_spec()),
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
    Ok(())
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
