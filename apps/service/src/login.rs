//! "Start the engine when I log in": the per-user entry each desktop OS starts programs from at
//! login, which `install-user-unit`, `uninstall-user-unit` and `show-user-unit` manage and the
//! app's Settings switch runs this binary for (ADR 0028):
//!
//! - Linux: the `systemd --user` unit ([`crate::unit`]);
//! - macOS: a LaunchAgent in `~/Library/LaunchAgents` ([`crate::launch_agent`]);
//! - Windows: a value under the per-user Run key ([`crate::run_key`]).
//!
//! Each module renders its entry on every OS, so the unit tests check all three here; only what
//! reads and writes the system picks the OS it runs on.

use std::path::{Path, PathBuf};

use anyhow::{Result, bail};
use filebridge_ipc::{LoginEntry, UnitAgent, UserUnit};

/// What the entry passes on to the service it starts.
#[derive(Debug, Clone, Copy, Default)]
pub struct Options<'a> {
    pub data_dir: Option<&'a Path>,
    pub rclone: Option<&'a Path>,
    /// Windows: a program without a console window that starts the service (the app, with
    /// `--start-engine`), which the Run value runs instead of this console program.
    pub launcher: Option<&'a Path>,
}

fn this_os() -> Result<LoginEntry> {
    if cfg!(target_os = "linux") {
        Ok(LoginEntry::SystemdUnit)
    } else if cfg!(target_os = "macos") {
        Ok(LoginEntry::LaunchAgent)
    } else if cfg!(windows) {
        Ok(LoginEntry::RunKey)
    } else {
        bail!("starting the engine at login is not available on this OS")
    }
}

fn no_launcher_here(o: &Options, kind: LoginEntry) -> Result<()> {
    if o.launcher.is_some() && kind != LoginEntry::RunKey {
        bail!("--launcher is for Windows only");
    }
    Ok(())
}

/// The entry as it is and as installing would write it, for `show-user-unit`.
pub fn show(o: &Options) -> Result<UserUnit> {
    let kind = this_os()?;
    no_launcher_here(o, kind)?;
    let exe = this_exe()?;
    match kind {
        LoginEntry::SystemdUnit => crate::unit::show(&exe, o),
        LoginEntry::LaunchAgent => crate::launch_agent::show(&exe, o),
        LoginEntry::RunKey => crate::run_key::show(&exe, o),
    }
}

/// Writes and enables the entry. With `start`, also starts the engine now (Linux, macOS; on
/// Windows the Run value only takes effect at the next login).
pub fn install(o: &Options, start: bool) -> Result<()> {
    let kind = this_os()?;
    no_launcher_here(o, kind)?;
    let exe = this_exe()?;
    let reach = match kind {
        LoginEntry::SystemdUnit => crate::unit::install(&exe, o, start)?,
        LoginEntry::LaunchAgent => crate::launch_agent::install(&exe, o, start)?,
        LoginEntry::RunKey => crate::run_key::install(&exe, o)?,
    };
    if let Some(hint) = agent_hint(kind, reach) {
        eprintln!("{hint}");
    }
    Ok(())
}

/// Stops (where the OS stops what it started), disables and removes the entry.
pub fn uninstall() -> Result<()> {
    match this_os()? {
        LoginEntry::SystemdUnit => crate::unit::uninstall(),
        LoginEntry::LaunchAgent => crate::launch_agent::uninstall(),
        LoginEntry::RunKey => crate::run_key::uninstall(),
    }
}

/// With [`UnitAgent::Missing`], SSH destinations that sign in with the agent fail under the
/// entry, though they work from this shell: says how to fix it.
pub fn agent_hint(kind: LoginEntry, reach: UnitAgent) -> Option<String> {
    if reach != UnitAgent::Missing {
        return None;
    }
    let fix = match kind {
        LoginEntry::SystemdUnit => format!(
            "After each login run `systemctl --user import-environment SSH_AUTH_SOCK` and \
             `systemctl --user restart {}`",
            crate::unit::UNIT_NAME
        ),
        LoginEntry::LaunchAgent => format!(
            "After each login run `launchctl setenv SSH_AUTH_SOCK \"$SSH_AUTH_SOCK\"` and \
             `launchctl kickstart -k gui/$(id -u)/{}`",
            crate::launch_agent::LABEL
        ),
        // The Run value starts with the session's environment: never missing there.
        LoginEntry::RunKey => return None,
    };
    Some(format!(
        "! the service can't reach your SSH agent (its socket changes at each login and the \
         login manager doesn't have it): SSH destinations that sign in with it will fail. \
         {fix}, or give those destinations filebridge's own key"
    ))
}

/// This binary, as the entry runs it. Refused where that path won't last: inside an AppImage
/// (a mount that is gone once it exits) or an app macOS runs from a randomised read-only copy
/// because it was opened where it was downloaded (App Translocation).
fn this_exe() -> Result<PathBuf> {
    if std::env::var_os("APPIMAGE").is_some() {
        bail!(
            "running from an AppImage, whose files are gone once it exits: install the .deb or .rpm to start the engine at login"
        );
    }
    let exe = std::env::current_exe()?.canonicalize()?;
    if exe.to_string_lossy().contains("/AppTranslocation/") {
        bail!(
            "macOS runs filebridge from a temporary copy (App Translocation): move filebridge.app to Applications, open it from there, and try again"
        );
    }
    Ok(plain_windows_path(exe))
}

/// `\\?\C:\…` → `C:\…` and `\\?\UNC\server\share\…` → `\\server\share\…`: what `canonicalize`
/// gives on Windows, in the form every program and the Run key take. Others unchanged.
pub fn plain_windows_path(p: PathBuf) -> PathBuf {
    let s = p.to_string_lossy();
    if let Some(rest) = s.strip_prefix(r"\\?\UNC\") {
        return PathBuf::from(format!(r"\\{rest}"));
    }
    if let Some(rest) = s.strip_prefix(r"\\?\")
        && rest.as_bytes().get(1) == Some(&b':')
    {
        return PathBuf::from(rest);
    }
    p
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verbatim_windows_paths_become_plain_ones() {
        for (verbatim, plain) in [
            (
                r"\\?\C:\Program Files\filebridge\filebridge-service.exe",
                r"C:\Program Files\filebridge\filebridge-service.exe",
            ),
            (r"\\?\UNC\nas\apps\fb.exe", r"\\nas\apps\fb.exe"),
            // A volume GUID path has no plain form: left as it is.
            (r"\\?\Volume{1234}\fb.exe", r"\\?\Volume{1234}\fb.exe"),
            ("/usr/bin/filebridge-service", "/usr/bin/filebridge-service"),
        ] {
            assert_eq!(
                plain_windows_path(PathBuf::from(verbatim)),
                PathBuf::from(plain)
            );
        }
    }

    #[test]
    fn the_agent_hint_says_how_to_fix_it_on_each_os() {
        let systemd = agent_hint(LoginEntry::SystemdUnit, UnitAgent::Missing).unwrap();
        assert!(systemd.contains("import-environment SSH_AUTH_SOCK"));
        let launchd = agent_hint(LoginEntry::LaunchAgent, UnitAgent::Missing).unwrap();
        assert!(launchd.contains("launchctl setenv SSH_AUTH_SOCK"));
        assert!(launchd.contains("gui/$(id -u)/app.filebridge.service"));
        assert_eq!(agent_hint(LoginEntry::RunKey, UnitAgent::Missing), None);
        for reach in [UnitAgent::None, UnitAgent::Pinned, UnitAgent::Manager] {
            assert_eq!(agent_hint(LoginEntry::SystemdUnit, reach), None);
        }
    }
}
