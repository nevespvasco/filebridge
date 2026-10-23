//! "Start the engine when I log in" (Settings): the per-user login entry (ADR 0028), a
//! `systemd --user` unit on Linux, a LaunchAgent on macOS, a Run key value on Windows. The
//! service binary installs and removes it itself (`install-user-unit`, `uninstall-user-unit`,
//! `show-user-unit --json`): the window only runs that binary, as it does to start the service,
//! and never writes the entry. It works with no service running, so it is not an ipc request.
//!
//! On Windows the window passes itself as `--launcher`: the Run value then starts the app with
//! `--start-engine` ([`crate::service::start_engine`]), which starts the service without the
//! console window the service binary would open, and exits.

use filebridge_ipc::{ErrorBody, UserUnit};
use serde::Serialize;

/// Whether the engine starts at login, and what turning it on or off does.
#[derive(Debug, Clone, PartialEq, Serialize, specta::Type)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Autostart {
    /// Not possible here: the switch is hidden, or shown off with the reason.
    Unsupported { reason: AutostartUnsupported },
    /// The login entry as it is, and what turning it on writes.
    Unit { unit: UserUnit },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum AutostartUnsupported {
    /// Not on this OS: phones.
    #[cfg_attr(
        any(target_os = "linux", target_os = "macos", windows),
        allow(dead_code)
    )]
    Os,
    /// The app runs from an AppImage, whose files are gone once it exits.
    AppImage,
    /// No `filebridge-service` next to the app, in `FILEBRIDGE_SERVICE` or on `PATH`.
    NoService,
    /// `systemctl --user` doesn't answer (no systemd user session).
    NoSystemd,
    /// `launchctl` doesn't answer for this user's desktop session (macOS).
    NoLaunchd,
}

fn failed(message: String) -> ErrorBody {
    ErrorBody {
        code: "autostart".into(),
        message,
    }
}

/// How things stand now.
pub async fn status() -> Result<Autostart, ErrorBody> {
    blocking(|| imp::status(&imp::Context::current())).await
}

/// Turns it on (installs and enables the unit; the engine running now keeps running) or off
/// (stops, disables and removes it; the window then starts the engine itself again). Answers
/// how things stand after.
pub async fn set(on: bool) -> Result<Autostart, ErrorBody> {
    blocking(move || {
        let cx = imp::Context::current();
        imp::set(&cx, on)?;
        imp::status(&cx)
    })
    .await
}

async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> Result<T, ErrorBody> + Send + 'static,
) -> Result<T, ErrorBody> {
    tauri::async_runtime::spawn_blocking(f)
        .await
        .map_err(|e| failed(e.to_string()))?
}

#[cfg(any(target_os = "linux", target_os = "macos", windows))]
mod imp {
    use std::path::PathBuf;
    use std::process::Command;

    use filebridge_ipc::LoginEntry;

    use super::{Autostart, AutostartUnsupported, ErrorBody, UserUnit, failed};

    /// What the answer depends on, apart from the entry itself.
    pub struct Context {
        pub service: Option<PathBuf>,
        pub appimage: bool,
        /// Windows: this app, which the Run value starts with `--start-engine`.
        pub launcher: Option<PathBuf>,
    }

    impl Context {
        pub fn current() -> Self {
            Self {
                service: crate::service::find_service_binary(),
                appimage: cfg!(target_os = "linux") && std::env::var_os("APPIMAGE").is_some(),
                launcher: if cfg!(windows) {
                    std::env::current_exe().ok()
                } else {
                    None
                },
            }
        }
    }

    /// Runs the service binary with `args` (and `--launcher`, for the commands that render the
    /// entry); its stdout, or its error message.
    fn run(cx: &Context, args: &[&str], renders: bool) -> Result<Vec<u8>, ErrorBody> {
        let bin = cx
            .service
            .as_ref()
            .ok_or_else(|| failed("filebridge-service not found".into()))?;
        let mut cmd = Command::new(bin);
        cmd.args(args);
        if let (true, Some(launcher)) = (renders, &cx.launcher) {
            cmd.arg("--launcher").arg(launcher);
        }
        #[cfg(windows)]
        {
            // No console window flashing up for each call.
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }
        let out = cmd
            .output()
            .map_err(|e| failed(format!("could not run {}: {e}", bin.display())))?;
        if !out.status.success() {
            let err = String::from_utf8_lossy(&out.stderr);
            let err = err.trim();
            return Err(failed(
                err.strip_prefix("Error: ").unwrap_or(err).to_string(),
            ));
        }
        Ok(out.stdout)
    }

    pub fn status(cx: &Context) -> Result<Autostart, ErrorBody> {
        let reason = if cx.appimage {
            Some(AutostartUnsupported::AppImage)
        } else if cx.service.is_none() {
            Some(AutostartUnsupported::NoService)
        } else {
            None
        };
        if let Some(reason) = reason {
            return Ok(Autostart::Unsupported { reason });
        }
        let out = run(cx, &["show-user-unit", "--json"], true)?;
        let unit: UserUnit = serde_json::from_slice(&out)
            .map_err(|e| failed(format!("filebridge-service show-user-unit: {e}")))?;
        if !unit.manager {
            let reason = match unit.kind {
                LoginEntry::LaunchAgent => AutostartUnsupported::NoLaunchd,
                LoginEntry::SystemdUnit | LoginEntry::RunKey => AutostartUnsupported::NoSystemd,
            };
            return Ok(Autostart::Unsupported { reason });
        }
        Ok(Autostart::Unit { unit })
    }

    pub fn set(cx: &Context, on: bool) -> Result<(), ErrorBody> {
        if on {
            // Not started now: the engine running now holds the data folder, so a second one
            // started by systemd or launchd would only fail. From the next login the OS starts it.
            run(cx, &["install-user-unit", "--no-start"], true)?;
        } else {
            run(cx, &["uninstall-user-unit"], false)?;
        }
        Ok(())
    }

    #[cfg(all(test, unix))]
    mod tests {
        use super::*;

        /// A stand-in for the service binary: logs its arguments, prints `unit` for
        /// `show-user-unit` and fails `install-user-unit` when asked to.
        fn fake_service(dir: &std::path::Path, unit: &str, install_fails: bool) -> PathBuf {
            use std::os::unix::fs::PermissionsExt;
            let bin = dir.join("filebridge-service");
            let log = dir.join("args");
            let fail = if install_fails {
                "echo 'Error: systemctl --user enable failed' >&2; exit 1"
            } else {
                "exit 0"
            };
            std::fs::write(
                &bin,
                format!(
                    "#!/bin/sh\n[ \"$1\" = probe ] && exit 0\necho \"$*\" >> '{}'\ncase \"$1\" in\n  show-user-unit) printf '%s' '{unit}' ;;\n  install-user-unit) {fail} ;;\nesac\n",
                    log.display()
                ),
            )
            .unwrap();
            std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
            // A process another test forks while the file is still open for writing keeps it
            // open until it execs, and running the file meanwhile fails with "Text file busy".
            for _ in 0..100 {
                match Command::new(&bin).arg("probe").status() {
                    Err(e) if e.raw_os_error() == Some(26) => {
                        std::thread::sleep(std::time::Duration::from_millis(10))
                    }
                    _ => break,
                }
            }
            bin
        }

        fn unit(manager: bool) -> UserUnit {
            UserUnit {
                kind: LoginEntry::SystemdUnit,
                path: "/home/demo/.config/systemd/user/filebridge-service.service".into(),
                exec: "/usr/bin/filebridge-service".into(),
                installed: false,
                enabled: false,
                current: false,
                manager,
                contents: "[Service]\nExecStart=\"/usr/bin/filebridge-service\" run\n".into(),
                agent: filebridge_ipc::UnitAgent::Missing,
            }
        }

        #[test]
        fn the_unit_comes_from_the_service_binary() {
            let dir = tempfile::tempdir().unwrap();
            let json = serde_json::to_string(&unit(true)).unwrap();
            let cx = Context {
                service: Some(fake_service(dir.path(), &json, false)),
                appimage: false,
                launcher: None,
            };
            assert_eq!(status(&cx).unwrap(), Autostart::Unit { unit: unit(true) });
            set(&cx, true).unwrap();
            set(&cx, false).unwrap();
            let args = std::fs::read_to_string(dir.path().join("args")).unwrap();
            assert_eq!(
                args,
                "show-user-unit --json\ninstall-user-unit --no-start\nuninstall-user-unit\n"
            );
        }

        /// Windows: the Run value starts this app, so the commands that render it get it.
        #[test]
        fn the_launcher_goes_to_the_commands_that_render_the_entry() {
            let dir = tempfile::tempdir().unwrap();
            let run_key = UserUnit {
                kind: LoginEntry::RunKey,
                path: r"HKEY_CURRENT_USER\Software\Microsoft\Windows\CurrentVersion\Run\filebridge-service".into(),
                exec: r"C:\Program Files\filebridge\filebridge-desktop.exe".into(),
                contents: r#""C:\Program Files\filebridge\filebridge-desktop.exe" --start-engine"#.into(),
                ..unit(true)
            };
            let json = serde_json::to_string(&run_key).unwrap();
            let cx = Context {
                service: Some(fake_service(dir.path(), &json, false)),
                appimage: false,
                launcher: Some("/opt/filebridge/filebridge-desktop".into()),
            };
            assert_eq!(status(&cx).unwrap(), Autostart::Unit { unit: run_key });
            set(&cx, true).unwrap();
            set(&cx, false).unwrap();
            let args = std::fs::read_to_string(dir.path().join("args")).unwrap();
            assert_eq!(
                args,
                "show-user-unit --json --launcher /opt/filebridge/filebridge-desktop\n\
                 install-user-unit --no-start --launcher /opt/filebridge/filebridge-desktop\n\
                 uninstall-user-unit\n"
            );
        }

        #[test]
        fn a_mac_without_a_desktop_session_says_launchd() {
            let dir = tempfile::tempdir().unwrap();
            let agent = UserUnit {
                kind: LoginEntry::LaunchAgent,
                ..unit(false)
            };
            let json = serde_json::to_string(&agent).unwrap();
            let cx = Context {
                service: Some(fake_service(dir.path(), &json, false)),
                appimage: false,
                launcher: None,
            };
            assert_eq!(
                status(&cx).unwrap(),
                Autostart::Unsupported {
                    reason: AutostartUnsupported::NoLaunchd
                }
            );
        }

        #[test]
        fn what_stops_it_is_said_before_anything_runs() {
            let dir = tempfile::tempdir().unwrap();
            let json = serde_json::to_string(&unit(false)).unwrap();
            let bin = fake_service(dir.path(), &json, true);
            let no_systemd = Context {
                service: Some(bin.clone()),
                appimage: false,
                launcher: None,
            };
            assert_eq!(
                status(&no_systemd).unwrap(),
                Autostart::Unsupported {
                    reason: AutostartUnsupported::NoSystemd
                }
            );
            let err = set(&no_systemd, true).unwrap_err();
            assert_eq!(err.code, "autostart");
            assert_eq!(err.message, "systemctl --user enable failed");
            let appimage = Context {
                service: Some(bin),
                appimage: true,
                launcher: None,
            };
            let missing = Context {
                service: None,
                appimage: false,
                launcher: None,
            };
            for (cx, reason) in [
                (appimage, AutostartUnsupported::AppImage),
                (missing, AutostartUnsupported::NoService),
            ] {
                assert_eq!(status(&cx).unwrap(), Autostart::Unsupported { reason });
            }
        }
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
mod imp {
    use super::{Autostart, AutostartUnsupported, ErrorBody, failed};

    pub struct Context;

    impl Context {
        pub fn current() -> Self {
            Self
        }
    }

    pub fn status(_: &Context) -> Result<Autostart, ErrorBody> {
        Ok(Autostart::Unsupported {
            reason: AutostartUnsupported::Os,
        })
    }

    pub fn set(_: &Context, _on: bool) -> Result<(), ErrorBody> {
        Err(failed(
            "starting the engine at login is not available on this system".into(),
        ))
    }
}
