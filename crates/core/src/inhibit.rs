//! Keeps the system awake while a job runs (docs/07). Best effort: if the OS mechanism is
//! missing, the send still runs and a warning is logged.
//!
//! - Linux: `systemd-inhibit --what=sleep:idle --mode=block sh -c 'read _'`. The child waits on a
//!   pipe from us, so it ends when the guard is dropped *or* when this process dies.
//! - macOS: `caffeinate -i -w <our pid>`, killed when the guard is dropped.
//! - Windows: a thread holding `SetThreadExecutionState(ES_CONTINUOUS | ES_SYSTEM_REQUIRED)`.

/// Held while sleep must stay blocked; dropping it releases the block.
pub struct SleepGuard {
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    child: Option<std::process::Child>,
    #[cfg(windows)]
    stop: Option<std::sync::mpsc::Sender<()>>,
}

impl SleepGuard {
    /// Whether the OS accepted the block.
    pub fn is_active(&self) -> bool {
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        return self.child.is_some();
        #[cfg(windows)]
        return self.stop.is_some();
        #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
        return false;
    }
}

#[cfg(target_os = "linux")]
pub fn inhibit(reason: &str) -> SleepGuard {
    use std::process::{Command, Stdio};
    let child = Command::new("systemd-inhibit")
        .args([
            "--what=sleep:idle",
            "--who=filebridge",
            &format!("--why={reason}"),
            "--mode=block",
            "sh",
            "-c",
            "read _",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
    match child {
        Ok(child) => SleepGuard { child: Some(child) },
        Err(e) => {
            tracing::warn!("cannot block sleep (systemd-inhibit): {e}");
            SleepGuard { child: None }
        }
    }
}

#[cfg(target_os = "macos")]
pub fn inhibit(_reason: &str) -> SleepGuard {
    use std::process::{Command, Stdio};
    let child = Command::new("caffeinate")
        .args(["-i", "-w", &std::process::id().to_string()])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
    match child {
        Ok(child) => SleepGuard { child: Some(child) },
        Err(e) => {
            tracing::warn!("cannot block sleep (caffeinate): {e}");
            SleepGuard { child: None }
        }
    }
}

#[cfg(windows)]
pub fn inhibit(_reason: &str) -> SleepGuard {
    use windows_sys::Win32::System::Power::{
        ES_CONTINUOUS, ES_SYSTEM_REQUIRED, SetThreadExecutionState,
    };
    let (tx, rx) = std::sync::mpsc::channel::<()>();
    let spawned = std::thread::Builder::new()
        .name("filebridge-inhibit".into())
        .spawn(move || {
            // The state belongs to this thread, so it lives until the guard is dropped.
            unsafe { SetThreadExecutionState(ES_CONTINUOUS | ES_SYSTEM_REQUIRED) };
            let _ = rx.recv();
            unsafe { SetThreadExecutionState(ES_CONTINUOUS) };
        });
    match spawned {
        Ok(_) => SleepGuard { stop: Some(tx) },
        Err(e) => {
            tracing::warn!("cannot block sleep: {e}");
            SleepGuard { stop: None }
        }
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
pub fn inhibit(_reason: &str) -> SleepGuard {
    tracing::warn!("blocking sleep is not supported on this OS");
    SleepGuard {}
}

impl Drop for SleepGuard {
    fn drop(&mut self) {
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        if let Some(mut child) = self.child.take() {
            // Linux: closing stdin ends `read`, which ends systemd-inhibit and releases the lock.
            drop(child.stdin.take());
            if !matches!(child.try_wait(), Ok(Some(_))) {
                std::thread::sleep(std::time::Duration::from_millis(50));
                if !matches!(child.try_wait(), Ok(Some(_))) {
                    let _ = child.kill();
                }
            }
            let _ = child.wait();
        }
        #[cfg(windows)]
        drop(self.stop.take());
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    #[test]
    fn guard_releases_on_drop() {
        let guard = super::inhibit("filebridge test");
        // systemd-inhibit may be missing (containers) or refused; the guard must still drop
        // cleanly either way.
        drop(guard);
    }
}
