//! Opening the engine through `filebridge-host`, plus id/name lookups for the commands.

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Result, bail};
use filebridge_core::{Engine, JobSummary, Profile};
use filebridge_host::{Access, Host, HostOptions};

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
    _host: Host,
}

impl App {
    /// Read-only commands skip the lock and rclone; commands that change state take the data
    /// directory's lock (refused while `filebridge-service` runs), and those that reach a
    /// destination also start rclone.
    pub async fn open(g: &Global, access: Access, with_transport: bool) -> Result<Self> {
        let host = Host::open(
            &HostOptions {
                data_dir: g.data_dir.clone(),
                rclone: g.rclone.clone(),
            },
            access,
            with_transport,
        )
        .await?;
        Ok(Self {
            engine: host.engine.clone(),
            _host: host,
        })
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
