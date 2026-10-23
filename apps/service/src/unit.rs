//! `systemd --user` unit (Linux): replaces running sends inside `tmux` (docs/07), and is what
//! the app's Settings switch "Start the engine when I log in" installs and removes.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use filebridge_ipc::UserUnit;

pub const UNIT_NAME: &str = "filebridge-service.service";

/// The unit file for `exe`, passing on explicit `--data-dir` / `--rclone`.
pub fn render(exe: &Path, data_dir: Option<&Path>, rclone: Option<&Path>) -> String {
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
    format!(
        "[Unit]\n\
         Description=filebridge background service (file transfers)\n\
         After=network-online.target\n\
         \n\
         [Service]\n\
         ExecStart={}\n\
         Restart=on-failure\n\
         RestartSec=5\n\
         \n\
         [Install]\n\
         WantedBy=default.target\n",
        cmd.join(" ")
    )
}

/// systemd quoting: double quotes, with `\` and `"` escaped; `%` doubled (specifier).
fn quote(p: &Path) -> String {
    let s = p.to_string_lossy();
    let escaped = s
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('%', "%%");
    format!("\"{escaped}\"")
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
    let contents = render(&exe, data_dir, rclone);
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
    })
}

pub fn install(data_dir: Option<&Path>, rclone: Option<&Path>, start: bool) -> Result<()> {
    if !cfg!(target_os = "linux") {
        bail!("the systemd user unit is Linux-only; on this OS the app starts the service");
    }
    let exe = this_exe()?;
    let path = unit_path()?;
    std::fs::create_dir_all(path.parent().expect("has a parent"))?;
    std::fs::write(&path, render(&exe, data_dir, rclone))
        .with_context(|| format!("write {}", path.display()))?;
    systemctl(&["daemon-reload"])?;
    if start {
        systemctl(&["enable", "--now", UNIT_NAME])?;
    } else {
        systemctl(&["enable", UNIT_NAME])?;
    }
    println!("installed {}", path.display());
    Ok(())
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
        );
        assert!(unit.contains(
            "ExecStart=\"/home/u/My Apps/filebridge-service\" --data-dir \"/data/50%%\" run\n"
        ));
        assert!(unit.contains("Restart=on-failure"));
        assert!(unit.contains("WantedBy=default.target"));
        assert!(!unit.contains("--rclone"));
    }
}
