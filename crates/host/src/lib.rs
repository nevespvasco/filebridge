//! Wiring shared by the binaries that host the engine (`apps/service`, `apps/cli`): the data
//! directory and its single-instance lock, the rclone sidecar, and the built-in plugins.
//!
//! This is the one place, besides those binaries, that may name concrete plugins (CLAUDE.md:
//! only the hosts wire implementations into the `Registry`).

mod access;
mod nextcloud;
mod operations;

use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use filebridge_core::{Engine, EngineOptions, Registry, Store};
use filebridge_integration_nextcloud::{Nextcloud, ShellConnector, SshConnector};
use filebridge_secrets::{KeyringStore, MemoryStore, SecretStore, StoreResolver};
use filebridge_transport_fake::FakeTransport;
use filebridge_transport_rclone::{Rcd, RcloneTransport, Strategy};

pub use operations::EngineOperations;

use nextcloud::NextcloudWizard;

pub const DB_FILE: &str = "filebridge.db";
pub const LOCK_FILE: &str = "filebridge.lock";

#[derive(Debug, Clone, Default)]
pub struct HostOptions {
    /// Default: the OS data directory (see [`default_data_dir`]).
    pub data_dir: Option<PathBuf>,
    /// Default: `rclone` on PATH, else `~/.local/bin/rclone`.
    pub rclone: Option<PathBuf>,
    /// The transport jobs use.
    pub transport: TransportChoice,
}

/// Which transport the engine gets.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum TransportChoice {
    /// rclone, started as a sidecar: what the app ships.
    #[default]
    Rclone,
    /// None: for commands that never touch a destination. Starts nothing.
    None,
    /// The local-disk fake (`transport` id `fake`): for tests and the UI's dev mode, no rclone.
    Fake,
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
    /// The Nextcloud plugin registered in the engine, for its profile wizard.
    nextcloud: Option<Arc<Nextcloud>>,
    /// Id of the transport registered in the engine, for profiles the wizards save.
    transport: Option<&'static str>,
    /// SSH sign-ins (with profiles' own keys from the keyring), for the key operations.
    ssh: Option<Arc<dyn ShellConnector>>,
    _rcd: Option<Arc<Rcd>>,
    _lock: Option<DataLock>,
}

impl Host {
    /// Opens the data directory with the built-in plugins and `opts.transport`.
    pub async fn open(opts: &HostOptions, access: Access) -> Result<Self> {
        let mut registry = Registry::new();
        let mut transport = None;
        // Profiles' passwords and keys (ADR 0004): the OS keyring, except with the fake
        // transport (development and tests), which must never touch the user's keyring.
        let secrets: Arc<dyn SecretStore> = match opts.transport {
            TransportChoice::Fake => Arc::new(MemoryStore::default()),
            _ => Arc::new(KeyringStore),
        };
        registry.set_secrets(secrets.clone());
        // Plugins only read, through the transport-api contract; they never hold the keyring.
        let resolver = Arc::new(StoreResolver(secrets.clone()));
        let rcd = match opts.transport {
            TransportChoice::Rclone => {
                let bin = find_rclone(opts.rclone.as_deref())?;
                let rcd = Arc::new(
                    Rcd::start(&bin)
                        .await
                        .with_context(|| format!("start {}", bin.display()))?,
                );
                registry.add_transport(Arc::new(
                    RcloneTransport::new(rcd.clone(), Strategy::PerFile)
                        .with_secrets(resolver.clone()),
                ));
                transport = Some(RcloneTransport::ID);
                Some(rcd)
            }
            TransportChoice::Fake => {
                registry.add_transport(Arc::new(FakeTransport::default()));
                transport = Some(FakeTransport::ID);
                None
            }
            TransportChoice::None => None,
        };
        // SSH sign-ins read a profile's own key (ADR 0012) from the same keyring.
        let ssh: Arc<dyn ShellConnector> = Arc::new(SshConnector::with_secrets(resolver));
        let nextcloud = Arc::new(Nextcloud::new(ssh.clone()));
        registry.add_integration(nextcloud.clone());
        let mut host = Self::with_registry(opts, access, registry)?;
        // The rcd starts unlimited: give it the saved bandwidth limit before anything is sent.
        host.engine
            .load_settings()
            .await
            .context("read the settings")?;
        host._rcd = rcd;
        host.nextcloud = Some(nextcloud);
        host.ssh = Some(ssh);
        host.transport = transport;
        Ok(host)
    }

    /// The Nextcloud profile wizard, run here: detects over SSH and saves the profile. `modes`
    /// overrides the default `750`/`640`.
    pub async fn detect_nextcloud(
        &self,
        req: filebridge_ipc::DetectNextcloudRequest,
        modes: Option<(String, String)>,
    ) -> Result<filebridge_ipc::DetectNextcloudResponse, filebridge_ipc::ErrorBody> {
        let wizard = self
            .nextcloud_wizard()
            .ok_or_else(|| filebridge_ipc::ErrorBody {
                code: "unsupported".into(),
                message: "this host has no Nextcloud plugin or no transport".into(),
            })?;
        wizard.run(&self.engine, req, modes).await
    }

    /// Every ipc operation against this host's engine and plugins, for `filebridge-service`.
    pub fn operations(&self) -> EngineOperations {
        EngineOperations::with_plugins(
            self.engine.clone(),
            self.nextcloud_wizard(),
            self.ssh.clone(),
        )
    }

    fn nextcloud_wizard(&self) -> Option<NextcloudWizard> {
        Some(NextcloudWizard {
            nextcloud: self.nextcloud.clone()?,
            transport: self.transport?,
        })
    }

    /// Opens the data directory with the plugins in `registry` (tests bring their own).
    pub fn with_registry(opts: &HostOptions, access: Access, registry: Registry) -> Result<Self> {
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
        let engine = Engine::new(store, registry, EngineOptions::default());
        Ok(Self {
            engine,
            data_dir: dir,
            nextcloud: None,
            transport: None,
            ssh: None,
            _rcd: None,
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
        // Other tests in this binary spawn processes; a child shares the lock's open file until
        // it execs, for a moment. Free again means within a second, not at once.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
        while let Err(e) = lock_data_dir(dir.path()) {
            assert!(
                std::time::Instant::now() < deadline,
                "free again after drop: {e}"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }
}
