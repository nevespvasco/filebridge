//! `filebridge-service`: owns the engine, SQLite, the rclone sidecar and the sleep inhibitor,
//! and serves `crates/ipc` on a user-only local socket (ADR 0008). Closing the app's window
//! never stops a send.
//!
//! Stopping the service (Ctrl-C, SIGTERM, logout) leaves running jobs as they are stored; the
//! next start resumes them.

mod unit;

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use filebridge_core::JobState;
use filebridge_host::{Access, Host, HostOptions, TransportChoice};
use filebridge_ipc::{Client, Endpoint, Operations};

#[derive(Parser)]
#[command(name = "filebridge-service", version, about)]
struct Cli {
    /// Where the database lives. Default: `$FILEBRIDGE_DATA_DIR`, else the OS data directory.
    #[arg(long, global = true, env = "FILEBRIDGE_DATA_DIR")]
    data_dir: Option<PathBuf>,
    /// rclone binary. Default: next to this program, `rclone` on PATH, or `~/.local/bin/rclone`.
    #[arg(long, global = true, env = "FILEBRIDGE_RCLONE")]
    rclone: Option<PathBuf>,
    /// Socket path. Default: `$XDG_RUNTIME_DIR/filebridge/service.sock` (a named pipe on
    /// Windows).
    #[arg(long, global = true, env = "FILEBRIDGE_SOCKET")]
    socket: Option<PathBuf>,
    /// Development and tests: send with the local-disk fake transport (id `fake`) instead of
    /// rclone. Profiles must use `transport: "fake"` and a local root. The variable takes
    /// `1`/`0`, `true`/`false`, `yes`/`no`.
    #[arg(
        long,
        global = true,
        env = "FILEBRIDGE_FAKE_TRANSPORT",
        hide = true,
        value_parser = clap::builder::BoolishValueParser::new()
    )]
    fake_transport: bool,
    #[command(subcommand)]
    cmd: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
    /// Run in the foreground (the default).
    Run,
    /// Whether the service is running, and its active jobs.
    Status,
    /// Linux: install and start a `systemd --user` unit that runs this binary.
    InstallUserUnit {
        /// Install and enable, but don't start it now.
        #[arg(long)]
        no_start: bool,
    },
    /// Linux: stop, disable and remove the `systemd --user` unit.
    UninstallUserUnit,
}

fn endpoint(cli: &Cli) -> Endpoint {
    match &cli.socket {
        Some(p) => Endpoint::Path(p.clone()),
        None => Endpoint::default_for_user(),
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_env("FILEBRIDGE_LOG")
                .unwrap_or_else(|_| "info".into()),
        )
        .with_writer(std::io::stderr)
        .init();
    let cli = Cli::parse();
    match cli.cmd {
        None | Some(Cmd::Run) => run(&cli).await,
        Some(Cmd::Status) => status(&cli).await,
        Some(Cmd::InstallUserUnit { no_start }) => {
            unit::install(cli.data_dir.as_deref(), cli.rclone.as_deref(), !no_start)
        }
        Some(Cmd::UninstallUserUnit) => unit::uninstall(),
    }
}

async fn run(cli: &Cli) -> Result<()> {
    // The lock comes first: it is what makes replacing a stale socket safe.
    let transport = if cli.fake_transport {
        TransportChoice::Fake
    } else {
        TransportChoice::Rclone
    };
    let host = Host::open(
        &HostOptions {
            data_dir: cli.data_dir.clone(),
            rclone: cli.rclone.clone(),
            transport,
        },
        Access::Exclusive,
    )
    .await?;
    let engine = host.engine.clone();
    let operations = Arc::new(host.operations());
    let endpoint = endpoint(cli);
    let listener = filebridge_ipc::bind(&endpoint)
        .with_context(|| format!("listen on {}", endpoint.display()))?;
    tracing::info!(
        data = %host.data_dir.display(),
        socket = %endpoint.display(),
        "filebridge-service {} ready",
        env!("CARGO_PKG_VERSION")
    );
    let resumed = engine.resume_unfinished().await?;
    if !resumed.is_empty() {
        tracing::info!("resumed {} interrupted jobs", resumed.len());
    }
    tokio::select! {
        r = filebridge_ipc::serve(listener, operations) => {
            r.context("serving ipc")?;
        }
        _ = shutdown_signal() => tracing::info!("stopping; running jobs resume on the next start"),
    }
    Ok(())
}

async fn shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        let mut term = signal(SignalKind::terminate()).expect("install SIGTERM handler");
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = term.recv() => {}
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}

async fn status(cli: &Cli) -> Result<()> {
    let endpoint = endpoint(cli);
    let client = match Client::connect(&endpoint).await {
        Ok(c) => c,
        Err(_) => {
            println!("not running (no service at {})", endpoint.display());
            std::process::exit(3);
        }
    };
    let pong = client.ping().await?;
    println!(
        "running: version {}, protocol {}, at {}",
        pong.version,
        pong.protocol,
        endpoint.display()
    );
    let filter = filebridge_ipc::JobFilter {
        groups: vec![filebridge_ipc::JobGroup::Active],
        ..Default::default()
    };
    let jobs = client.list_jobs(filter, 50, 0).await?;
    let active: Vec<_> = jobs
        .items
        .iter()
        .filter(|j| j.state.is_running() || j.state == JobState::Paused)
        .collect();
    if active.is_empty() {
        println!("no active jobs");
    }
    for j in active {
        println!(
            "  {}  {:<14} {}/{} files",
            &j.id[..8],
            j.state.as_str(),
            j.counts.done,
            j.counts.files
        );
    }
    Ok(())
}
