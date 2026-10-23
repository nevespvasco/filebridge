//! Opening the engine through `filebridge-host`, plus id/name lookups for the commands.

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Result, bail};
use filebridge_core::{Engine, JobSummary, Profile};
use filebridge_host::{Access, EngineOperations, Host, HostOptions, TransportChoice};
use filebridge_ipc::Operations;

/// Options shared by every engine command.
#[derive(clap::Args, Clone)]
pub struct Global {
    /// Where the database lives. Default: `$FILEBRIDGE_DATA_DIR`, else the OS data directory.
    #[arg(long, global = true, env = "FILEBRIDGE_DATA_DIR")]
    pub data_dir: Option<PathBuf>,
    /// rclone binary. Default: `filebridge-rclone` (the installed app's) or `rclone` next to this
    /// program, `rclone` on PATH, or `~/.local/bin/rclone`.
    #[arg(long, global = true, env = "FILEBRIDGE_RCLONE")]
    pub rclone: Option<PathBuf>,
}

pub struct App {
    /// For what isn't an operation: raw source paths, events, waiting, resuming at startup.
    pub engine: Arc<Engine>,
    /// Every operation, through the same interface the window uses (`filebridge_ipc::Operations`).
    pub ops: EngineOperations,
    pub host: Host,
}

impl App {
    /// Read-only commands skip the lock and rclone; commands that change state take the data
    /// directory's lock (refused while `filebridge-service` runs), and those that reach a
    /// destination also start rclone.
    pub async fn open(g: &Global, access: Access, with_transport: bool) -> Result<Self> {
        let transport = if with_transport {
            TransportChoice::Rclone
        } else {
            TransportChoice::None
        };
        let host = Host::open(
            &HostOptions {
                data_dir: g.data_dir.clone(),
                cache_dir: None,
                rclone: g.rclone.clone(),
                transport,
            },
            access,
        )
        .await?;
        Ok(Self {
            engine: host.engine.clone(),
            ops: host.operations(),
            host,
        })
    }

    /// A profile by exact id or name.
    pub async fn profile(&self, key: &str) -> Result<Profile> {
        let profiles = self.ops.list_profiles().await?;
        if let Some(p) = profiles.iter().find(|p| p.id == key || p.name == key) {
            return Ok(p.clone());
        }
        let names: Vec<_> = profiles.iter().map(|p| p.name.as_str()).collect();
        bail!("no profile {key:?} (have: {})", names.join(", "))
    }

    /// A job by id or unique id prefix. Straight from the engine: `list_jobs` over ipc pages by
    /// 1000, and the prefix must be unique among all jobs.
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
