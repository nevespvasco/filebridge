//! `systemd --user` unit (Linux): replaces running sends inside `tmux` (docs/07), and is what
//! the app's Settings switch "Start the engine when I log in" installs and removes.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use filebridge_ipc::{UnitAgent, UserUnit};

pub const UNIT_NAME: &str = "filebridge-service.service";

/// The unit file for `exe`, passing on explicit `--data-dir` / `--rclone`, and the SSH agent's
/// socket when [`Agent`] pins it.
pub fn render(
    exe: &Path,
    data_dir: Option<&Path>,
    rclone: Option<&Path>,
    agent: Option<&str>,
) -> String {
    let mut cmd = vec![quote(exe)];
    if let Some(d) = data_dir {
        cmd.push("--data-dir".into());
        cmd.push(quote(d));
    }
    if let Some(r) = rclone {
        cmd.push("--rclone".into());
        cmd.push(quote(r));
    }
    cmd.push("run".into());
    let env = agent
        .map(|sock| format!("Environment=\"SSH_AUTH_SOCK={sock}\"\n"))
        .unwrap_or_default();
    format!(
        "[Unit]\n\
         Description=filebridge background service (file transfers)\n\
         After=network-online.target\n\
         \n\
         [Service]\n\
         {env}\
         ExecStart={}\n\
         Restart=on-failure\n\
         RestartSec=5\n\
         \n\
         [Install]\n\
         WantedBy=default.target\n",
        cmd.join(" ")
    )
}

/// systemd quoting: double quotes around [`escape`].
fn quote(p: &Path) -> String {
    format!("\"{}\"", escape(&p.to_string_lossy()))
}

/// For inside systemd double quotes: `\` and `"` escaped; `%` doubled (specifier).
fn escape(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('%', "%%")
}

/// How the engine the unit starts finds the SSH agent. The unit's process starts with the user
/// manager's environment, which often lacks the session's `SSH_AUTH_SOCK`, and even when it has
/// it the unit may start at login before the agent sets it. So a socket at a fixed per-user path
/// (`$XDG_RUNTIME_DIR/…`, as GNOME Keyring, gcr, gpg-agent and `ssh-agent.socket` use, or under
/// home) is written into the unit, as `%t/…` or `%h/…`. The path only, never a key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Agent {
    pub reach: UnitAgent,
    /// With [`UnitAgent::Pinned`]: the socket as a unit value, `%t/keyring/ssh`.
    pin: Option<String>,
}

impl Agent {
    fn current() -> Self {
        let var = |name| std::env::var_os(name).map(PathBuf::from);
        // A variable left over from a dead agent is no agent.
        let sock = var("SSH_AUTH_SOCK").filter(|p| p.exists());
        Self::decide(
            sock.as_deref(),
            var("XDG_RUNTIME_DIR").as_deref(),
            var("HOME").as_deref(),
            &manager_environment(),
        )
    }

    fn decide(
        sock: Option<&Path>,
        runtime_dir: Option<&Path>,
        home: Option<&Path>,
        manager_env: &str,
    ) -> Self {
        let Some(sock) = sock else {
            return Self {
                reach: UnitAgent::None,
                pin: None,
            };
        };
        let pin = [("%t", runtime_dir), ("%h", home)]
            .into_iter()
            .find_map(|(specifier, base)| {
                let rel = sock.strip_prefix(base?).ok()?;
                let plain = rel.components().next().is_some()
                    && rel
                        .components()
                        .all(|c| matches!(c, std::path::Component::Normal(_)));
                plain
                    .then(|| rel.to_str())
                    .flatten()
                    .map(|rel| format!("{specifier}/{}", escape(rel)))
            });
        let reach = if pin.is_some() {
            UnitAgent::Pinned
        } else if manager_env.lines().any(|l| l.starts_with("SSH_AUTH_SOCK=")) {
            UnitAgent::Manager
        } else {
            UnitAgent::Missing
        };
        Self { reach, pin }
    }
}

fn unit_path() -> Result<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .context("cannot find the config directory")?;
    Ok(base.join("systemd/user").join(UNIT_NAME))
}

fn systemctl(args: &[&str]) -> Result<()> {
    let status = std::process::Command::new("systemctl")
        .arg("--user")
        .args(args)
        .status()
        .context("run systemctl --user")?;
    if !status.success() {
        bail!("systemctl --user {} failed ({status})", args.join(" "));
    }
    Ok(())
}

/// This binary, as the unit runs it. Refused inside an AppImage: what runs there lives in a
/// mount that is gone once the AppImage exits.
fn this_exe() -> Result<PathBuf> {
    if std::env::var_os("APPIMAGE").is_some() {
        bail!(
            "running from an AppImage, whose files are gone once it exits: install the .deb or .rpm to start the engine at login"
        );
    }
    Ok(std::env::current_exe()?.canonicalize()?)
}

/// The unit as it is and as installing would write it, for `show-user-unit`.
pub fn show(data_dir: Option<&Path>, rclone: Option<&Path>) -> Result<UserUnit> {
    if !cfg!(target_os = "linux") {
        bail!("the systemd user unit is Linux-only");
    }
    let exe = this_exe()?;
    let path = unit_path()?;
    let agent = Agent::current();
    let contents = render(&exe, data_dir, rclone, agent.pin.as_deref());
    let on_disk = std::fs::read_to_string(&path).ok();
    let answers = |args: &[&str]| {
        std::process::Command::new("systemctl")
            .arg("--user")
            .args(args)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
    };
    // Succeeds only with a user manager to talk to.
    let systemd = answers(&["show-environment"]);
    Ok(UserUnit {
        path: path.display().to_string(),
        exec: exe.display().to_string(),
        installed: on_disk.is_some(),
        enabled: systemd && answers(&["is-enabled", "--quiet", UNIT_NAME]),
        current: on_disk.as_deref() == Some(contents.as_str()),
        systemd,
        contents,
        agent: agent.reach,
    })
}

pub fn install(data_dir: Option<&Path>, rclone: Option<&Path>, start: bool) -> Result<()> {
    if !cfg!(target_os = "linux") {
        bail!("the systemd user unit is Linux-only; on this OS the app starts the service");
    }
    let exe = this_exe()?;
    let path = unit_path()?;
    std::fs::create_dir_all(path.parent().expect("has a parent"))?;
    let agent = Agent::current();
    std::fs::write(&path, render(&exe, data_dir, rclone, agent.pin.as_deref()))
        .with_context(|| format!("write {}", path.display()))?;
    systemctl(&["daemon-reload"])?;
    if start {
        systemctl(&["enable", "--now", UNIT_NAME])?;
    } else {
        systemctl(&["enable", UNIT_NAME])?;
    }
    println!("installed {}", path.display());
    if let Some(hint) = agent_hint(agent.reach) {
        eprintln!("{hint}");
    }
    Ok(())
}

/// `systemctl --user show-environment`: what the unit's process starts with. Empty if it fails.
fn manager_environment() -> String {
    std::process::Command::new("systemctl")
        .args(["--user", "show-environment"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .unwrap_or_default()
}

/// With [`UnitAgent::Missing`], SSH destinations that sign in with the agent fail under the
/// unit, though they work from this shell: says how to fix it.
pub fn agent_hint(reach: UnitAgent) -> Option<String> {
    (reach == UnitAgent::Missing).then(|| {
        format!(
            "! the service can't reach your SSH agent (its socket changes at each login and \
             systemd's user manager doesn't have it): SSH destinations that sign in with it \
             will fail. After each login run `systemctl --user import-environment \
             SSH_AUTH_SOCK` and `systemctl --user restart {UNIT_NAME}`, or give those \
             destinations filebridge's own key"
        )
    })
}

pub fn uninstall() -> Result<()> {
    let path = unit_path()?;
    if !path.exists() {
        println!("not installed ({} does not exist)", path.display());
        return Ok(());
    }
    // Stopping first leaves running jobs stored as running; they resume on the next start.
    let _ = systemctl(&["disable", "--now", UNIT_NAME]);
    std::fs::remove_file(&path)?;
    systemctl(&["daemon-reload"])?;
    println!("removed {}", path.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unit_quotes_paths_and_passes_options() {
        let unit = render(
            Path::new("/home/u/My Apps/filebridge-service"),
            Some(Path::new("/data/50%")),
            None,
            None,
        );
        assert!(unit.contains(
            "ExecStart=\"/home/u/My Apps/filebridge-service\" --data-dir \"/data/50%%\" run\n"
        ));
        assert!(unit.contains("Restart=on-failure"));
        assert!(unit.contains("WantedBy=default.target"));
        assert!(!unit.contains("--rclone"));
        assert!(!unit.contains("Environment="));
    }

    fn decide(sock: Option<&str>, manager_env: &str) -> Agent {
        Agent::decide(
            sock.map(Path::new),
            Some(Path::new("/run/user/1000")),
            Some(Path::new("/home/u")),
            manager_env,
        )
    }

    #[test]
    fn a_fixed_per_user_agent_socket_is_written_into_the_unit() {
        let gnome = decide(Some("/run/user/1000/keyring/ssh"), "");
        assert_eq!(gnome.reach, UnitAgent::Pinned);
        let unit = render(
            Path::new("/usr/bin/filebridge-service"),
            None,
            None,
            gnome.pin.as_deref(),
        );
        assert!(unit.contains(
            "[Service]\nEnvironment=\"SSH_AUTH_SOCK=%t/keyring/ssh\"\nExecStart=\"/usr/bin/filebridge-service\" run\n"
        ));
        // Pinned even when the user manager has it now: at login the unit may start first.
        let home = decide(Some("/home/u/.1password/agent.sock"), "SSH_AUTH_SOCK=/x\n");
        assert_eq!(home.pin.as_deref(), Some("%h/.1password/agent.sock"));
        // `%` would be a specifier, `"` would end the value.
        let odd = decide(Some("/run/user/1000/50%/a\"b"), "");
        assert_eq!(odd.pin.as_deref(), Some("%t/50%%/a\\\"b"));
    }

    #[test]
    fn an_agent_that_moves_at_each_login_comes_from_the_user_manager_or_is_missing() {
        for sock in [
            "/tmp/ssh-XXXXabcd/agent.1234",
            "/run/user/1000/../1001/ssh",
            "/run/user/1000",
        ] {
            let manager = decide(Some(sock), "PATH=/usr/bin\nSSH_AUTH_SOCK=/tmp/x\n");
            assert_eq!(manager.reach, UnitAgent::Manager, "{sock}");
            assert_eq!(manager.pin, None);
            assert_eq!(agent_hint(manager.reach), None);
            let missing = decide(Some(sock), "PATH=/usr/bin\n");
            assert_eq!(missing.reach, UnitAgent::Missing, "{sock}");
            assert!(
                agent_hint(missing.reach)
                    .unwrap()
                    .contains("import-environment SSH_AUTH_SOCK")
            );
        }
        // No agent here either: nothing the unit could be given, nothing to warn about.
        let none = decide(None, "");
        assert_eq!((none.reach, none.pin), (UnitAgent::None, None));
        assert_eq!(agent_hint(UnitAgent::None), None);
    }
}
