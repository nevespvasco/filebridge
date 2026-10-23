//! Windows: the value `filebridge-service` under the per-user Run key
//! (`HKEY_CURRENT_USER\Software\Microsoft\Windows\CurrentVersion\Run`), which Windows runs at each
//! login ([`crate::login`]). No administrator rights, nothing to install.
//!
//! This binary is a console program, so run from there it would open a console window at every
//! login and keep it open. The app passes `--launcher` with itself instead: a GUI program that,
//! started with `--start-engine`, starts this service without a window (as it does when it
//! opens) and exits. Without a launcher (installed from a terminal) the value runs this binary.
//!
//! Task Manager's "Startup apps" turns a Run value off without deleting it, in
//! `…\Explorer\StartupApproved\Run`: read for `enabled`, cleared when installing.

use std::path::Path;

use anyhow::{Context, Result};
use filebridge_ipc::{LoginEntry, START_ENGINE_ARG, UnitAgent, UserUnit};

use crate::login::Options;

pub const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
#[cfg_attr(not(windows), allow(dead_code))]
pub const APPROVED_KEY: &str =
    r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run";
pub const VALUE: &str = "filebridge-service";

/// The Run value's command line: the launcher with `--start-engine`, or this binary with
/// `run`, passing on explicit `--data-dir` / `--rclone`.
pub fn command(
    exe: &Path,
    launcher: Option<&Path>,
    data_dir: Option<&Path>,
    rclone: Option<&Path>,
) -> String {
    let mut args = vec![quote(&launcher.unwrap_or(exe).to_string_lossy())];
    if launcher.is_some() {
        args.push(START_ENGINE_ARG.into());
    }
    if let Some(d) = data_dir {
        args.push("--data-dir".into());
        args.push(quote(&d.to_string_lossy()));
    }
    if let Some(r) = rclone {
        args.push("--rclone".into());
        args.push(quote(&r.to_string_lossy()));
    }
    if launcher.is_none() {
        args.push("run".into());
    }
    args.join(" ")
}

/// Quoted for a Windows command line (the rules programs built with the Microsoft C runtime and
/// Rust parse it with): backslashes are literal except before a `"`, where they are doubled.
pub fn quote(arg: &str) -> String {
    let mut out = String::from("\"");
    let mut backslashes = 0;
    for c in arg.chars() {
        match c {
            '\\' => backslashes += 1,
            '"' => {
                out.extend(std::iter::repeat_n('\\', backslashes * 2 + 1));
                out.push('"');
                backslashes = 0;
            }
            _ => {
                out.extend(std::iter::repeat_n('\\', backslashes));
                out.push(c);
                backslashes = 0;
            }
        }
    }
    // Before the closing quote, so it isn't escaped.
    out.extend(std::iter::repeat_n('\\', backslashes * 2));
    out.push('"');
    out
}

/// A `StartupApproved\Run` value: its first byte is even when the entry is on (`02 00 …`) and
/// odd when Task Manager turned it off (`03 00 …`, then when).
#[cfg_attr(not(windows), allow(dead_code))]
pub fn approved_off(bytes: &[u8]) -> bool {
    bytes.first().is_some_and(|b| b & 1 == 1)
}

/// Where the value lives, for the window and the messages.
pub fn display_path() -> String {
    format!(r"HKEY_CURRENT_USER\{RUN_KEY}\{VALUE}")
}

/// Its own environment is the session's (Explorer's), as the window's is: whatever agent the
/// window reaches, the engine started at login reaches too.
fn current_agent() -> UnitAgent {
    if std::env::var_os("SSH_AUTH_SOCK").is_some() {
        UnitAgent::Manager
    } else {
        UnitAgent::None
    }
}

pub fn show(exe: &Path, o: &Options) -> Result<UserUnit> {
    let contents = command(exe, o.launcher, o.data_dir, o.rclone);
    let on_disk = registry::read()?;
    Ok(UserUnit {
        kind: LoginEntry::RunKey,
        path: display_path(),
        exec: o.launcher.unwrap_or(exe).display().to_string(),
        installed: on_disk.is_some(),
        enabled: on_disk.is_some() && !registry::turned_off(),
        current: on_disk.as_deref() == Some(contents.as_str()),
        manager: true,
        contents,
        agent: current_agent(),
    })
}

/// Writes the value (and turns it back on if Task Manager turned it off). It takes effect at
/// the next login: nothing starts now.
pub fn install(exe: &Path, o: &Options) -> Result<UnitAgent> {
    registry::write(&command(exe, o.launcher, o.data_dir, o.rclone))
        .with_context(|| format!("write {}", display_path()))?;
    println!("installed {}", display_path());
    Ok(current_agent())
}

/// Deletes the value. The engine it started keeps running.
pub fn uninstall() -> Result<()> {
    if registry::delete().with_context(|| format!("delete {}", display_path()))? {
        println!("removed {}", display_path());
    } else {
        println!("not installed ({} does not exist)", display_path());
    }
    Ok(())
}

#[cfg(windows)]
mod registry {
    use std::io::ErrorKind;

    use winreg::RegKey;
    use winreg::enums::{HKEY_CURRENT_USER, KEY_SET_VALUE};

    use super::{APPROVED_KEY, RUN_KEY, VALUE, approved_off};

    fn hkcu() -> RegKey {
        RegKey::predef(HKEY_CURRENT_USER)
    }

    /// The value, or `None` when it (or the key) is missing.
    pub fn read() -> std::io::Result<Option<String>> {
        let key = match hkcu().open_subkey(RUN_KEY) {
            Ok(key) => key,
            Err(e) if e.kind() == ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e),
        };
        match key.get_value::<String, _>(VALUE) {
            Ok(v) => Ok(Some(v)),
            Err(e) if e.kind() == ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e),
        }
    }

    pub fn turned_off() -> bool {
        hkcu()
            .open_subkey(APPROVED_KEY)
            .and_then(|key| key.get_raw_value(VALUE))
            .is_ok_and(|v| approved_off(&v.bytes[..]))
    }

    pub fn write(command: &str) -> std::io::Result<()> {
        let (key, _) = hkcu().create_subkey(RUN_KEY)?;
        key.set_value(VALUE, &command.to_owned())?;
        if let Ok(approved) = hkcu().open_subkey_with_flags(APPROVED_KEY, KEY_SET_VALUE) {
            // Missing is the usual case.
            let _ = approved.delete_value(VALUE);
        }
        Ok(())
    }

    /// Whether there was a value to delete.
    pub fn delete() -> std::io::Result<bool> {
        if let Ok(approved) = hkcu().open_subkey_with_flags(APPROVED_KEY, KEY_SET_VALUE) {
            let _ = approved.delete_value(VALUE);
        }
        let key = match hkcu().open_subkey_with_flags(RUN_KEY, KEY_SET_VALUE) {
            Ok(key) => key,
            Err(e) if e.kind() == ErrorKind::NotFound => return Ok(false),
            Err(e) => return Err(e),
        };
        match key.delete_value(VALUE) {
            Ok(()) => Ok(true),
            Err(e) if e.kind() == ErrorKind::NotFound => Ok(false),
            Err(e) => Err(e),
        }
    }
}

/// No registry off Windows; [`crate::login`] never gets here there.
#[cfg(not(windows))]
mod registry {
    fn unsupported() -> std::io::Error {
        std::io::Error::new(std::io::ErrorKind::Unsupported, "no registry on this OS")
    }

    pub fn read() -> std::io::Result<Option<String>> {
        Err(unsupported())
    }

    pub fn turned_off() -> bool {
        false
    }

    pub fn write(_command: &str) -> std::io::Result<()> {
        Err(unsupported())
    }

    pub fn delete() -> std::io::Result<bool> {
        Err(unsupported())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SERVICE: &str = r"C:\Program Files\filebridge\filebridge-service.exe";
    const APP: &str = r"C:\Program Files\filebridge\filebridge-desktop.exe";

    #[test]
    fn the_app_starts_the_service_without_a_window() {
        assert_eq!(
            command(Path::new(SERVICE), Some(Path::new(APP)), None, None),
            r#""C:\Program Files\filebridge\filebridge-desktop.exe" --start-engine"#
        );
        assert_eq!(
            command(
                Path::new(SERVICE),
                Some(Path::new(APP)),
                Some(Path::new(r"D:\filebridge data\")),
                Some(Path::new(r"C:\Tools\rclone.exe")),
            ),
            r#""C:\Program Files\filebridge\filebridge-desktop.exe" --start-engine --data-dir "D:\filebridge data\\" --rclone "C:\Tools\rclone.exe""#
        );
    }

    #[test]
    fn without_a_launcher_the_value_runs_the_service() {
        assert_eq!(
            command(Path::new(SERVICE), None, None, None),
            r#""C:\Program Files\filebridge\filebridge-service.exe" run"#
        );
    }

    #[test]
    fn quoting_follows_the_windows_command_line_rules() {
        for (arg, quoted) in [
            (r"C:\a b\c.exe", r#""C:\a b\c.exe""#),
            // A trailing backslash would escape the closing quote.
            (r"D:\", r#""D:\\""#),
            (r"\\nas\share\", r#""\\nas\share\\""#),
            // Not in a path, but in an argument: the quote and the backslashes before it escaped.
            (r#"say "hi""#, r#""say \"hi\"""#),
            (r#"a\"b"#, r#""a\\\"b""#),
            ("", r#""""#),
        ] {
            assert_eq!(quote(arg), quoted, "{arg}");
        }
    }

    #[test]
    fn task_manager_turning_it_off_is_seen() {
        assert!(!approved_off(&[2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]));
        assert!(!approved_off(&[6, 0, 0, 0]));
        assert!(approved_off(&[
            3, 0, 0, 0, 0x10, 0x2f, 0xa1, 0x7c, 0x3e, 0x44, 0xdb, 0x01
        ]));
        assert!(!approved_off(&[]));
    }

    #[test]
    fn the_value_is_named_in_full() {
        assert_eq!(
            display_path(),
            r"HKEY_CURRENT_USER\Software\Microsoft\Windows\CurrentVersion\Run\filebridge-service"
        );
    }
}
