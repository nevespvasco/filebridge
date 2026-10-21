//! Opening the engine: data directory, SQLite store, rclone sidecar and the plugin registry.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use filebridge_core::{Engine, EngineOptions, JobSummary, Profile, Registry, Store};
use filebridge_integration_nextcloud::Nextcloud;
use filebridge_transport_rclone::{Rcd, RcloneTransport, Strategy};

/// Options shared by every engine command.
#[derive(clap::Args, Clone)]
pub struct Global {
    /// Where the database lives. Default: `$FILEBRIDGE_DATA_DIR`, else the OS data directory.
    #[arg(long, global = true, env = "FILEBRIDGE_DATA_DIR")]
    pub data_dir: Option<PathBuf>,
    /// rclone binary. Default: `rclone` on PATH, else `~/.local/bin/rclone`.
    #[arg(long, global = true, env = "FILEBRIDGE_RCLONE")]
    pub rclone: Option<PathBuf>,
}

pub struct App {
    pub engine: Arc<Engine>,
    /// Keeps the rclone sidecar alive for as long as the engine needs it.
    _rcd: Option<Arc<Rcd>>,
}

impl App {
    /// `with_transport: false` skips starting rclone, for commands that only read the store.
    pub async fn open(g: &Global, with_transport: bool) -> Result<Self> {
        let dir = match &g.data_dir {
            Some(d) => d.clone(),
            None => default_data_dir()?,
        };
        std::fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
        let store = Store::open(&dir.join("filebridge.db"))
            .with_context(|| format!("open the database in {}", dir.display()))?;
        let mut registry = Registry::new();
        let rcd = if with_transport {
            let bin = find_rclone(g.rclone.as_deref())?;
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
        Ok(Self { engine, _rcd: rcd })
    }

    /// A profile by exact id or name.
    pub async fn profile(&self, key: &str) -> Result<Profile> {
        let profiles = self.engine.list_profiles().await?;
        if let Some(p) = profiles.iter().find(|p| p.id == key || p.name == key) {
            return Ok(p.clone());
        }
        let names: Vec<_> = profiles.iter().map(|p| p.name.as_str()).collect();
        bail!("no profile {key:?} (have: {})", names.join(", "))
    }

    /// A job by id or unique id prefix.
    pub async fn job(&self, key: &str) -> Result<JobSummary> {
        let jobs = self.engine.list_jobs(10_000, 0).await?;
        let found: Vec<_> = jobs.into_iter().filter(|j| j.id.starts_with(key)).collect();
        match found.len() {
            1 => Ok(found.into_iter().next().expect("one")),
            0 => bail!("no job {key:?}"),
            n => bail!("{n} jobs start with {key:?}; give more characters"),
        }
    }
}

fn default_data_dir() -> Result<PathBuf> {
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

fn find_rclone(explicit: Option<&Path>) -> Result<PathBuf> {
    if let Some(p) = explicit {
        return Ok(p.to_path_buf());
    }
    let exe = if cfg!(windows) {
        "rclone.exe"
    } else {
        "rclone"
    };
    if let Some(paths) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&paths) {
            let candidate = dir.join(exe);
            if candidate.is_file() {
                return Ok(candidate);
            }
        }
    }
    if let Some(home) = std::env::var_os("HOME") {
        let candidate = PathBuf::from(home).join(".local/bin").join(exe);
        if candidate.is_file() {
            return Ok(candidate);
        }
    }
    bail!("rclone not found; install it or pass --rclone")
}
