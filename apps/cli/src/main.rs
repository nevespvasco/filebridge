//! filebridge CLI: the same engine as the app, without a UI (servers, scripts).
//!
//! Engine commands (`profile`, `send`, `settings`, `jobs`, …) run `core` in-process. The Phase 0
//! commands (`scan`, `fixture`, `bench`) stay for benchmarking rename-in-transit.

mod app;
mod bench;
mod fixture;
mod prepare;
mod profile_cmd;
mod report_cmd;
mod send_cmd;
mod settings_cmd;

use std::path::PathBuf;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "filebridge", version, about)]
struct Cli {
    #[command(flatten)]
    global: app::Global,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Destinations: add, list, test, remove.
    #[command(subcommand)]
    Profile(profile_cmd::ProfileCmd),
    /// Analyse and send files or folders to a profile.
    Send(send_cmd::SendArgs),
    /// Check the names already in a destination folder and fix them there ("Repair
    /// destination"). Files are only renamed, never changed or deleted
    Repair(send_cmd::RepairArgs),
    /// Name issues of a job (pending ones unless --all).
    Issues {
        job: String,
        #[arg(long)]
        rule: Option<String>,
        #[arg(long)]
        all: bool,
    },
    /// Decide one name issue.
    Decide {
        job: String,
        issue: i64,
        #[arg(value_enum)]
        decision: send_cmd::DecisionArg,
        /// The new name, with `edit`.
        #[arg(long)]
        name: Option<String>,
    },
    /// One decision for every pending name issue of a job (or of one rule). Not `edit`.
    DecideAll {
        job: String,
        #[arg(value_enum)]
        decision: send_cmd::DecisionArg,
        /// Only the issues of this rule (e.g. `collision`).
        #[arg(long)]
        rule: Option<String>,
    },
    /// Send a job that is ready (after decisions).
    Start { job: String },
    /// Continue a paused or interrupted job; without an id, every interrupted job.
    Resume { job: Option<String> },
    /// Stop a job for good. Nothing is deleted.
    Cancel { job: String },
    /// The app-wide bandwidth limit and send window (default: show them).
    Settings {
        #[command(subcommand)]
        cmd: Option<settings_cmd::SettingsCmd>,
    },
    /// Recent jobs.
    Jobs {
        #[arg(long, default_value_t = 20)]
        limit: u32,
    },
    /// One job's details.
    Job { job: String },
    /// A finished job's report: every file, both paths, status and checksum. To stdout by default.
    /// `report diff` compares two; `report check` checks a sealed one.
    Report(report_cmd::ReportArgs),
    /// List the names the NFC rule would correct. Read-only.
    Scan {
        source: PathBuf,
        /// Print every corrected name with its bytes.
        #[arg(long)]
        show: bool,
    },
    /// Create a synthetic test tree where some names are NFD. Refuses a non-empty folder.
    Fixture(fixture::Args),
    /// Compare a plain `sync/copy` (baseline) with plan A (one `copyfile` per file, under the
    /// corrected name) and the engine. Never writes to the source.
    Bench(bench::Args),
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_env("FILEBRIDGE_LOG")
                .unwrap_or_else(|_| "warn".into()),
        )
        .with_writer(std::io::stderr)
        .init();
    let cli = Cli::parse();
    let g = &cli.global;
    match cli.cmd {
        Cmd::Profile(cmd) => profile_cmd::run(g, cmd).await,
        Cmd::Send(args) => send_cmd::send(g, args).await,
        Cmd::Repair(args) => send_cmd::repair(g, args).await,
        Cmd::Issues { job, rule, all } => send_cmd::issues(g, &job, rule, all).await,
        Cmd::Decide {
            job,
            issue,
            decision,
            name,
        } => send_cmd::decide(g, &job, issue, decision, name).await,
        Cmd::DecideAll {
            job,
            decision,
            rule,
        } => send_cmd::decide_all(g, &job, rule, decision).await,
        Cmd::Start { job } => send_cmd::start(g, &job).await,
        Cmd::Resume { job } => send_cmd::resume(g, job).await,
        Cmd::Cancel { job } => send_cmd::cancel(g, &job).await,
        Cmd::Settings { cmd } => settings_cmd::run(g, cmd).await,
        Cmd::Jobs { limit } => send_cmd::jobs(g, limit).await,
        Cmd::Job { job } => send_cmd::show(g, &job).await,
        Cmd::Report(args) => report_cmd::run(g, args).await,
        Cmd::Scan { source, show } => prepare::print_scan(&source, show),
        Cmd::Fixture(args) => fixture::run(&args),
        Cmd::Bench(args) => bench::run(args, g.rclone.as_deref()).await,
    }
}
