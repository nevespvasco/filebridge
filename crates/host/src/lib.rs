//! Wiring shared by the binaries that host the engine (`apps/service`, `apps/cli`): the data
//! directory and its single-instance lock, the rclone sidecar, and the built-in plugins.
//!
//! This is the one place, besides those binaries, that may name concrete plugins (CLAUDE.md:
//! only the hosts wire implementations into the `Registry`).

mod nextcloud;

use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use filebridge_core::{Engine, EngineOptions, Registry, Store};
use filebridge_integration_nextcloud::Nextcloud;
use filebridge_transport_rclone::{Rcd, RcloneTransport, Strategy};

pub use nextcloud::{NextcloudExtensions, detect_nextcloud};

pub const DB_FILE: &str = "filebridge.db";
pub const LOCK_FILE: &str = "filebridge.lock";

#[derive(Debug, Clone, Default)]
pub struct HostOptions {
    /// Default: the OS data directory (see [`default_data_dir`]).
    pub data_dir: Option<PathBuf>,
    /// Default: `rclone` on PATH, else `~/.local/bin/rclone`.
    pub rclone: Option<PathBuf>,
}

/// What the caller will do with the engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Access {
    /// Only reads (listing jobs, issues). No lock; SQLite allows concurrent readers.
    Read,
    /// Changes state or sends. Takes the data directory's lock, so only one engine runs on it.
    Exclusive,
}

/// Held for as long as one engine owns the data directory.
pub struct DataLock {
    _file: File,
}

/// Takes the data directory's lock without waiting.
pub fn lock_data_dir(dir: &Path) -> Result<DataLock> {
    let path = dir.join(LOCK_FILE);
    let file = File::options()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&path)
        .with_context(|| format!("open {}", path.display()))?;
    match file.try_lock() {
        Ok(()) => Ok(DataLock { _file: file }),
        Err(std::fs::TryLockError::WouldBlock) => bail!(
            "another filebridge engine is using {} (is filebridge-service running? \
             use the app, or stop the service first)",
            dir.display()
        ),
        Err(std::fs::TryLockError::Error(e)) => {
            Err(e).with_context(|| format!("lock {}", path.display()))
        }
    }
}

/// An engine with its store, sidecar and (for [`Access::Exclusive`]) lock.
pub struct Host {
    pub engine: Arc<Engine>,
    pub data_dir: PathBuf,
    _rcd: Option<Arc<Rcd>>,
    _lock: Option<DataLock>,
}

impl Host {
    /// `with_transport: false` skips starting rclone (commands that never touch a destination).
    pub async fn open(opts: &HostOptions, access: Access, with_transport: bool) -> Result<Self> {
        let dir = match &opts.data_dir {
            Some(d) => d.clone(),
            None => default_data_dir()?,
        };
        std::fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
        let lock = match access {
            Access::Exclusive => Some(lock_data_dir(&dir)?),
            Access::Read => None,
        };
        let store = Store::open(&dir.join(DB_FILE))
            .with_context(|| format!("open the database in {}", dir.display()))?;
        let mut registry = Registry::new();
        let rcd = if with_transport {
            let bin = find_rclone(opts.rclone.as_deref())?;
            let rcd = Arc::new(
                Rcd::start(&bin)
                    .await
                    .with_context(|| format!("start {}", bin.display()))?,
            );
            registry.add_transport(Arc::new(RcloneTransport::new(
                rcd.clone(),
                Strategy::PerFile,
            )));
            Some(rcd)
        } else {
            None
        };
        registry.add_integration(Arc::new(Nextcloud::with_ssh()));
        let engine = Engine::new(store, registry, EngineOptions::default());
        Ok(Self {
            engine,
            data_dir: dir,
            _rcd: rcd,
            _lock: lock,
        })
    }
}

pub fn default_data_dir() -> Result<PathBuf> {
    let home = || std::env::var_os("HOME").map(PathBuf::from);
    let dir = if cfg!(target_os = "macos") {
        home().map(|h| h.join("Library/Application Support/filebridge"))
    } else if cfg!(windows) {
        std::env::var_os("APPDATA").map(|a| PathBuf::from(a).join("filebridge"))
    } else {
        std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .or_else(|| home().map(|h| h.join(".local/share")))
            .map(|d| d.join("filebridge"))
    };
    dir.context("cannot find a data directory; pass --data-dir")
}

/// The rclone binary: `explicit`, else a sidecar next to the current executable (installed
/// app), else `rclone` on PATH, else `~/.local/bin/rclone`.
pub fn find_rclone(explicit: Option<&Path>) -> Result<PathBuf> {
    if let Some(p) = explicit {
        return Ok(p.to_path_buf());
    }
    let exe = if cfg!(windows) {
        "rclone.exe"
    } else {
        "rclone"
    };
    let beside = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join(exe)));
    let on_path = std::env::var_os("PATH")
        .map(|paths| {
            std::env::split_paths(&paths)
                .map(|d| d.join(exe))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let local_bin = std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/bin").join(exe));
    beside
        .into_iter()
        .chain(on_path)
        .chain(local_bin)
        .find(|c| c.is_file())
        .context("rclone not found; install it or pass --rclone")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_lock_is_exclusive_and_released_on_drop() {
        let dir = tempfile::tempdir().unwrap();
        let first = lock_data_dir(dir.path()).unwrap();
        let err = lock_data_dir(dir.path()).err().expect("second lock fails");
        assert!(err.to_string().contains("another filebridge engine"));
        drop(first);
        lock_data_dir(dir.path()).expect("free again after drop");
    }
}
