//! filebridge CLI: the same engine as the app, without a UI (servers, scripts).
//!
//! Engine commands (`profile`, `send`, `schedule`, `settings`, `jobs`, …) run `core` in-process.
//! The Phase 0 commands (`scan`, `fixture`, `bench`) stay for benchmarking rename-in-transit.
//! `completions` and `man` print what an install puts in place for the shell and `man`
//! (apps/cli/README.md).

mod app;
mod bench;
mod fixture;
mod prepare;
mod profile_cmd;
mod report_cmd;
mod schedule_cmd;
mod send_cmd;
mod settings_cmd;

use std::path::{Path, PathBuf};

use anyhow::Context;
use clap::{CommandFactory, Parser, Subcommand};

/// The crate version and `git describe` of the build (build.rs).
const VERSION: &str = env!("FILEBRIDGE_VERSION");

const AFTER_HELP: &str = "\
Job ids accept a unique prefix (the 8 characters `filebridge jobs` prints).

Environment:
  FILEBRIDGE_DATA_DIR  the data folder (default: the OS one, ~/.local/share/filebridge)
  FILEBRIDGE_RCLONE    the rclone binary (default: see --rclone)
  FILEBRIDGE_LOG       log filter, e.g. debug or rclone=debug (default: warn)

Changing commands refuse to run while filebridge-service holds the data folder.";

#[derive(Parser)]
#[command(
    name = "filebridge",
    version = VERSION,
    about = "Send files and folders to SFTP, Nextcloud, Immich or a local folder, fixing names \
             in transit; never changes or deletes the source",
    after_help = AFTER_HELP
)]
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
    /// Scheduled sends: add, list, remove, run now (filebridge-service runs them at their
    /// times).
    #[command(subcommand)]
    Schedule(schedule_cmd::ScheduleCmd),
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
    /// Print the shell completion script, e.g. `filebridge completions bash >
    /// ~/.local/share/bash-completion/completions/filebridge`.
    Completions {
        #[arg(value_enum)]
        shell: clap_complete::Shell,
    },
    /// Print the man page, or write one per command (filebridge.1, filebridge-send.1, …) into a
    /// folder, e.g. `filebridge man --dir ~/.local/share/man/man1`.
    Man {
        /// Write every page here (created if missing) instead of printing filebridge.1.
        #[arg(long)]
        dir: Option<PathBuf>,
    },
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
        Cmd::Schedule(cmd) => schedule_cmd::run(g, cmd).await,
        Cmd::Settings { cmd } => settings_cmd::run(g, cmd).await,
        Cmd::Jobs { limit } => send_cmd::jobs(g, limit).await,
        Cmd::Job { job } => send_cmd::show(g, &job).await,
        Cmd::Report(args) => report_cmd::run(g, args).await,
        Cmd::Scan { source, show } => prepare::print_scan(&source, show),
        Cmd::Fixture(args) => fixture::run(&args),
        Cmd::Bench(args) => bench::run(args, g.rclone.as_deref()).await,
        Cmd::Completions { shell } => {
            clap_complete::generate(
                shell,
                &mut Cli::command(),
                "filebridge",
                &mut std::io::stdout(),
            );
            Ok(())
        }
        Cmd::Man { dir: None } => {
            clap_mangen::Man::new(Cli::command()).render(&mut std::io::stdout())?;
            Ok(())
        }
        Cmd::Man { dir: Some(dir) } => write_man_pages(&dir),
    }
}

/// One page per command and subcommand, named the way `man filebridge-profile-add-sftp` finds it.
fn write_man_pages(dir: &Path) -> anyhow::Result<()> {
    std::fs::create_dir_all(dir).with_context(|| format!("create {}", dir.display()))?;
    let mut cmd = Cli::command();
    // Gives each subcommand its full name, `filebridge-profile-add-sftp`.
    cmd.build();
    write_page(&cmd, dir)
}

/// `cmd`'s page and its subcommands', without `help`'s, which only repeat them.
fn write_page(cmd: &clap::Command, dir: &Path) -> anyhow::Result<()> {
    let name = cmd.get_display_name().unwrap_or(cmd.get_name());
    let path = dir.join(format!("{name}.1"));
    let mut out = std::fs::File::create(&path)
        .map(std::io::BufWriter::new)
        .with_context(|| format!("write {}", path.display()))?;
    clap_mangen::Man::new(cmd.clone()).render(&mut out)?;
    std::io::Write::flush(&mut out)?;
    for sub in cmd.get_subcommands() {
        if sub.get_name() != "help" && !sub.is_hide_set() {
            write_page(sub, dir)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_command_line_is_consistent() {
        Cli::command().debug_assert();
    }

    #[test]
    fn the_version_names_the_build() {
        assert!(VERSION.starts_with(env!("CARGO_PKG_VERSION")));
        let cli = Cli::command();
        assert_eq!(cli.get_version(), Some(VERSION));
    }

    #[test]
    fn completions_cover_every_shell() {
        for shell in [
            clap_complete::Shell::Bash,
            clap_complete::Shell::Zsh,
            clap_complete::Shell::Fish,
            clap_complete::Shell::PowerShell,
            clap_complete::Shell::Elvish,
        ] {
            let mut out = Vec::new();
            clap_complete::generate(shell, &mut Cli::command(), "filebridge", &mut out);
            let script = String::from_utf8(out).unwrap();
            assert!(script.contains("decide-all"), "{shell}");
        }
    }

    #[test]
    fn man_pages_are_one_per_command() {
        let dir = tempfile::tempdir().unwrap();
        write_man_pages(dir.path()).unwrap();
        for page in [
            "filebridge.1",
            "filebridge-send.1",
            "filebridge-profile-add-sftp.1",
        ] {
            let text = std::fs::read_to_string(dir.path().join(page)).unwrap();
            assert!(text.starts_with(".ie"), "{page}");
        }
        let top = std::fs::read_to_string(dir.path().join("filebridge.1")).unwrap();
        assert!(top.contains("FILEBRIDGE_DATA_DIR"));
        let pages = std::fs::read_dir(dir.path()).unwrap().count();
        assert!(!dir.path().join("filebridge-help.1").exists());
        assert!(pages > 20, "{pages}");
    }
}
