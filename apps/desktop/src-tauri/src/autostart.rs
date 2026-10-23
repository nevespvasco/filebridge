//! "Start the engine when I log in" (Settings). On Linux it is the `systemd --user` unit, which
//! the service binary installs and removes itself (`install-user-unit`, `uninstall-user-unit`,
//! `show-user-unit --json`): the window only runs that binary, as it does to start the service,
//! and never writes the unit. It works with no service running, so it is not an ipc request.
//!
//! TODO(windows): a per-user Run key or Task Scheduler entry, written by the service binary in
//! the same way. TODO(macos): a LaunchAgent in `~/Library/LaunchAgents`. Until then both answer
//! `Unsupported { reason: Os }` and the switch is not shown.

use filebridge_ipc::{ErrorBody, UserUnit};
use serde::Serialize;

/// Whether the engine starts at login, and what turning it on or off does.
#[derive(Debug, Clone, PartialEq, Serialize, specta::Type)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Autostart {
    /// Not possible here: the switch is hidden, or shown off with the reason.
    Unsupported { reason: AutostartUnsupported },
    /// The systemd user unit as it is, and the file turning it on writes.
    Unit { unit: UserUnit },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum AutostartUnsupported {
    /// Not done yet on this OS (Windows, macOS), and never on a phone.
    #[cfg_attr(target_os = "linux", allow(dead_code))]
    Os,
    /// The app runs from an AppImage, whose files are gone once it exits.
    AppImage,
    /// No `filebridge-service` next to the app, in `FILEBRIDGE_SERVICE` or on `PATH`.
    NoService,
    /// `systemctl --user` doesn't answer (no systemd user session).
    NoSystemd,
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

#[cfg(target_os = "linux")]
mod imp {
    use std::path::PathBuf;
    use std::process::Command;

    use super::{Autostart, AutostartUnsupported, ErrorBody, UserUnit, failed};

    /// What the answer depends on, apart from the unit itself.
    pub struct Context {
        pub service: Option<PathBuf>,
        pub appimage: bool,
    }

    impl Context {
        pub fn current() -> Self {
            Self {
                service: crate::service::find_service_binary(),
                appimage: std::env::var_os("APPIMAGE").is_some(),
            }
        }
    }

    /// Runs the service binary with `args`; its stdout, or its error message.
    fn run(cx: &Context, args: &[&str]) -> Result<Vec<u8>, ErrorBody> {
        let bin = cx
            .service
            .as_ref()
            .ok_or_else(|| failed("filebridge-service not found".into()))?;
        let out = Command::new(bin)
            .args(args)
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
        let out = run(cx, &["show-user-unit", "--json"])?;
        let unit: UserUnit = serde_json::from_slice(&out)
            .map_err(|e| failed(format!("filebridge-service show-user-unit: {e}")))?;
        if !unit.systemd {
            return Ok(Autostart::Unsupported {
                reason: AutostartUnsupported::NoSystemd,
            });
        }
        Ok(Autostart::Unit { unit })
    }

    pub fn set(cx: &Context, on: bool) -> Result<(), ErrorBody> {
        if on {
            // Not `--now`: the engine running now holds the data folder, so a second one started
            // by systemd would only fail. From the next login systemd starts it.
            run(cx, &["install-user-unit", "--no-start"])?;
        } else {
            run(cx, &["uninstall-user-unit"])?;
        }
        Ok(())
    }

    #[cfg(test)]
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
                    "#!/bin/sh\necho \"$*\" >> '{}'\ncase \"$1\" in\n  show-user-unit) printf '%s' '{unit}' ;;\n  install-user-unit) {fail} ;;\nesac\n",
                    log.display()
                ),
            )
            .unwrap();
            std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
            bin
        }

        fn unit(systemd: bool) -> UserUnit {
            UserUnit {
                path: "/home/demo/.config/systemd/user/filebridge-service.service".into(),
                exec: "/usr/bin/filebridge-service".into(),
                installed: false,
                enabled: false,
                current: false,
                systemd,
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

        #[test]
        fn what_stops_it_is_said_before_anything_runs() {
            let dir = tempfile::tempdir().unwrap();
            let json = serde_json::to_string(&unit(false)).unwrap();
            let bin = fake_service(dir.path(), &json, true);
            let no_systemd = Context {
                service: Some(bin.clone()),
                appimage: false,
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
            };
            let missing = Context {
                service: None,
                appimage: false,
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

#[cfg(not(target_os = "linux"))]
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
            "starting the engine at login is not available on this system yet".into(),
        ))
    }
}
