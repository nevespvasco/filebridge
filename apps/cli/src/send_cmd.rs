//! `filebridge send`, `repair`, `issues`, `decide`, `decide-all`, `start`, `resume`, `jobs`,
//! `job`, `cancel`. Operations go through `filebridge_ipc::Operations`, like the window's.
//!
//! The CLI runs the engine in-process: a send stops when the command exits (Ctrl-C pauses it
//! cleanly) and continues with `filebridge resume`. The background service (ADR 0008) removes
//! that limit for the desktop app.

use std::io::{IsTerminal, Write};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Result, bail};
use filebridge_core::{
    AnalysisSummary, DecisionInput, Engine, Event, IssueDecision, JobErrorCode, JobState,
    SendOptions,
};
use tokio::sync::broadcast::error::RecvError;

use crate::app::{App, Global};
use crate::profile_cmd::print_preflight;
use filebridge_host::Access;
use filebridge_ipc::Operations;

#[derive(clap::Args)]
pub struct SendArgs {
    /// Files and folders to send. Each one keeps its name at the destination.
    #[arg(required = true)]
    paths: Vec<PathBuf>,
    /// Destination profile (name or id).
    #[arg(long)]
    to: String,
    /// Compare SHA-256 of every file at the end and resend any that differ.
    #[arg(long)]
    verify: bool,
    #[arg(long, default_value_t = 8)]
    concurrency: u32,
    /// A folder under the destination's root to send into (created by the send).
    #[arg(long)]
    into: Option<String>,
    /// Don't ask before sending.
    #[arg(long, short)]
    yes: bool,
}

#[derive(clap::Args)]
pub struct RepairArgs {
    /// Destination profile (name or id).
    #[arg(long)]
    to: String,
    /// The folder to check, under the destination's root (default: the whole destination).
    #[arg(long)]
    dir: Option<String>,
    /// Don't ask before renaming.
    #[arg(long, short)]
    yes: bool,
}

#[derive(clap::ValueEnum, Clone, Copy)]
pub enum DecisionArg {
    /// Use the proposed name.
    Accept,
    /// Keep the original name.
    Keep,
    /// Leave the file (or folder) out.
    Skip,
    /// Use the name given with --name.
    Edit,
}

pub async fn send(g: &Global, args: SendArgs) -> Result<()> {
    let app = App::open(g, Access::Exclusive, true).await?;
    let profile = app.profile(&args.to).await?;
    let job = app
        .engine
        .create_job(
            args.paths,
            &profile.id,
            SendOptions {
                verify: args.verify,
                concurrency: args.concurrency,
                target_dir: args.into,
            },
        )
        .await?;
    eprintln!("job {} → {}", &job[..8], profile.name);
    eprintln!("analysing …");
    let summary = app.ops.analyse(job.clone()).await?;
    print_analysis(&summary);
    if summary.state == JobState::AwaitingApproval {
        println!();
        println!(
            "{} names need a decision. See them with `filebridge issues {}` and decide with \
             `filebridge decide {} <issue> accept|keep|skip|edit` (or all at once with \
             `filebridge decide-all {} accept|keep|skip`); then `filebridge start {}`.",
            summary.pending_decisions,
            &job[..8],
            &job[..8],
            &job[..8],
            &job[..8]
        );
        return Ok(());
    }
    if summary.preflight.iter().any(|p| p.has_failures()) {
        bail!(
            "the destination checks failed; fix them and run `filebridge start {}`",
            &job[..8]
        );
    }
    if !args.yes && !confirm("Send now?")? {
        println!(
            "not sent; start later with `filebridge start {}`",
            &job[..8]
        );
        return Ok(());
    }
    app.ops.start(job.clone()).await?;
    follow(&app.engine, &job).await
}

/// "Repair destination" (docs/04): checks the names already in a destination folder with the
/// same rules as a send, then renames them there.
pub async fn repair(g: &Global, args: RepairArgs) -> Result<()> {
    let app = App::open(g, Access::Exclusive, true).await?;
    let profile = app.profile(&args.to).await?;
    let job = app
        .ops
        .create_repair_job(profile.id.clone(), args.dir.clone())
        .await?;
    let folder = args.dir.as_deref().unwrap_or("/");
    eprintln!("job {} → repair {folder} on {}", &job[..8], profile.name);
    eprintln!("reading the folder …");
    let summary = app.ops.analyse(job.clone()).await?;
    print_analysis(&summary);
    if summary.rules.is_empty() {
        println!("every name is fine; nothing to rename");
        return Ok(());
    }
    if summary.state == JobState::AwaitingApproval {
        println!();
        println!(
            "{} names need a decision: `filebridge issues {j}`, then `filebridge decide {j} …` or `filebridge decide-all {j} …`, then `filebridge start {j}` to rename.",
            summary.pending_decisions,
            j = &job[..8],
        );
        return Ok(());
    }
    let changes: u64 = summary.rules.iter().map(|r| r.total).sum();
    let question = format!(
        "Rename up to {changes} names on {} in {folder}? Files are only renamed, never changed or deleted.",
        profile.name
    );
    if !args.yes && !confirm(&question)? {
        println!(
            "nothing renamed; rename later with `filebridge start {}`",
            &job[..8]
        );
        return Ok(());
    }
    app.ops.start(job.clone()).await?;
    follow(&app.engine, &job).await
}

pub async fn start(g: &Global, job: &str) -> Result<()> {
    let app = App::open(g, Access::Exclusive, true).await?;
    let job = app.job(job).await?.id;
    app.ops.start(job.clone()).await?;
    follow(&app.engine, &job).await
}

/// Resumes one job, or every job that was running when the last command stopped.
pub async fn resume(g: &Global, job: Option<String>) -> Result<()> {
    let app = App::open(g, Access::Exclusive, true).await?;
    let ids = match job {
        Some(key) => {
            let j = app.job(&key).await?;
            if j.state.is_running() {
                app.engine.resume_unfinished().await?;
            } else {
                app.ops.resume(j.id.clone()).await?;
            }
            vec![j.id]
        }
        None => app.engine.resume_unfinished().await?,
    };
    if ids.is_empty() {
        println!("nothing to resume");
    }
    for id in ids {
        eprintln!("resuming {}", &id[..8]);
        follow(&app.engine, &id).await?;
    }
    Ok(())
}

pub async fn issues(g: &Global, job: &str, rule: Option<String>, all: bool) -> Result<()> {
    let app = App::open(g, Access::Read, false).await?;
    let job = app.job(job).await?;
    for r in app.ops.rule_counts(job.id.clone(), None).await? {
        println!("{:<18} {:>7} names, {} pending", r.rule, r.total, r.pending);
    }
    let mut after = None;
    loop {
        let page = app
            .ops
            .issues(job.id.clone(), rule.clone(), after, 500, None)
            .await?;
        let Some(last) = page.last() else { break };
        after = Some(last.id);
        for i in page {
            if !all && i.decision != IssueDecision::Pending {
                continue;
            }
            println!(
                "#{:<6} {:<10} {:<9} {}\n         {} → {}{}",
                i.id,
                i.rule,
                i.decision.as_str(),
                i.path,
                i.before_text,
                i.after_text,
                if i.before_text == i.after_text {
                    format!("   (bytes {} → {})", hex(&i.before), hex(&i.after))
                } else {
                    String::new()
                }
            );
        }
    }
    Ok(())
}

pub async fn decide(
    g: &Global,
    job: &str,
    issue: i64,
    decision: DecisionArg,
    name: Option<String>,
) -> Result<()> {
    let app = App::open(g, Access::Exclusive, false).await?;
    let job = app.job(job).await?;
    let input = match (decision, name) {
        (DecisionArg::Accept, _) => DecisionInput::Accept,
        (DecisionArg::Keep, _) => DecisionInput::KeepOriginal,
        (DecisionArg::Skip, _) => DecisionInput::Skip,
        (DecisionArg::Edit, Some(n)) => DecisionInput::Edit(n),
        (DecisionArg::Edit, None) => bail!("edit needs --name"),
    };
    app.ops.decide(job.id.clone(), issue, input).await?;
    let job = app.ops.job(job.id).await?;
    println!("job {} is {}", &job.id[..8], job.state.as_str());
    Ok(())
}

/// One decision for every pending issue of a job, or of one rule.
pub async fn decide_all(
    g: &Global,
    job: &str,
    rule: Option<String>,
    decision: DecisionArg,
) -> Result<()> {
    let app = App::open(g, Access::Exclusive, false).await?;
    let job = app.job(job).await?;
    let input = match decision {
        DecisionArg::Accept => DecisionInput::Accept,
        DecisionArg::Keep => DecisionInput::KeepOriginal,
        DecisionArg::Skip => DecisionInput::Skip,
        DecisionArg::Edit => bail!("an edited name applies to one issue; use `decide`"),
    };
    let n = app
        .ops
        .decide_all(job.id.clone(), rule, input, None)
        .await?;
    let job = app.ops.job(job.id).await?;
    println!(
        "{n} issues decided; job {} is {}",
        &job.id[..8],
        job.state.as_str()
    );
    Ok(())
}

pub async fn jobs(g: &Global, limit: u32) -> Result<()> {
    let app = App::open(g, Access::Read, false).await?;
    let page = app
        .ops
        .list_jobs(filebridge_ipc::JobFilter::default(), limit, 0)
        .await?;
    for j in page.items {
        println!(
            "{}  {:<24} {:>6}/{:<6} files  {:>10}  {}",
            &j.id[..8],
            j.state.as_str(),
            j.counts.done,
            j.counts.files,
            human_bytes(j.counts.bytes),
            j.sources.join(", ")
        );
    }
    Ok(())
}

pub async fn show(g: &Global, job: &str) -> Result<()> {
    let app = App::open(g, Access::Read, false).await?;
    let j = app.job(job).await?;
    println!("job       {}", j.id);
    println!("state     {}", j.state.as_str());
    if let Some(e) = &j.error {
        println!("message   {e}");
    }
    println!("sources   {}", j.sources.join(", "));
    let c = &j.counts;
    println!(
        "files     {} done, {} pending, {} failed, {} skipped of {} ({} / {})",
        c.done,
        c.pending,
        c.failed,
        c.skipped,
        c.files,
        human_bytes(c.bytes_done),
        human_bytes(c.bytes)
    );
    if j.options.verify {
        println!("verified  {} ok, {} differ", c.verified_ok, c.mismatch);
    }
    if j.attempt > 0 {
        println!("retries   {} in a row", j.attempt);
    }
    Ok(())
}

pub async fn cancel(g: &Global, job: &str) -> Result<()> {
    let app = App::open(g, Access::Exclusive, false).await?;
    let j = app.job(job).await?;
    app.ops.cancel(j.id.clone()).await?;
    println!("cancelled {} (nothing was deleted)", &j.id[..8]);
    Ok(())
}

fn print_analysis(s: &AnalysisSummary) {
    println!(
        "{} files in {} folders, {}",
        s.files,
        s.dirs,
        human_bytes(s.bytes)
    );
    for r in &s.rules {
        let what = if r.pending > 0 {
            format!("{} need a decision", r.pending)
        } else {
            "fixed automatically".into()
        };
        println!("  {:<18} {:>7} names — {what}", r.rule, r.total);
    }
    if s.skipped_symlinks > 0 {
        println!("  {} symbolic links are not sent", s.skipped_symlinks);
    }
    for e in s.scan_errors.iter().take(10) {
        println!("  ! cannot read {e}");
    }
    if s.scan_errors.len() > 10 {
        println!(
            "  ! … and {} more unreadable paths",
            s.scan_errors.len() - 10
        );
    }
    for p in &s.preflight {
        print_preflight(p);
        if let Some(free) = p.free_bytes
            && free < s.bytes
        {
            println!(
                "  ! only {} free for {}",
                human_bytes(free),
                human_bytes(s.bytes)
            );
        }
    }
}

/// Prints progress until the job stops. Ctrl-C pauses the job instead of killing the transfer.
async fn follow(engine: &Arc<Engine>, job_id: &str) -> Result<()> {
    let mut rx = engine.subscribe();
    let tty = std::io::stderr().is_terminal();
    let mut last_line = Instant::now() - Duration::from_secs(1);
    let mut wrote_progress = false;
    let mut warned_space = false;
    let short = &job_id[..8];
    let wait = engine.wait(job_id);
    tokio::pin!(wait);
    loop {
        tokio::select! {
            state = &mut wait => {
                if wrote_progress && tty {
                    eprintln!();
                }
                let state = state?;
                let job = engine.job(job_id).await?;
                match state {
                    JobState::Completed => println!("✓ done: {} files, {}", job.counts.done, human_bytes(job.counts.bytes_done)),
                    JobState::CompletedWithWarnings => println!("! done with warnings: {}", job.error.unwrap_or_default()),
                    JobState::Paused if job.error_code == Some(JobErrorCode::DestinationFull) => println!(
                        "! the destination is full ({}); free space there, then continue with \
                         `filebridge resume {short}`", job.error.unwrap_or_default()),
                    JobState::Paused => println!("paused; continue with `filebridge resume {short}`"),
                    other => println!("job {short} is {}{}", other.as_str(),
                        job.error.map(|e| format!(": {e}")).unwrap_or_default()),
                }
                return Ok(());
            }
            _ = tokio::signal::ctrl_c() => {
                if wrote_progress && tty {
                    eprintln!();
                }
                eprintln!("pausing …");
                engine.pause(job_id).await?;
                println!("paused; continue with `filebridge resume {short}`");
                return Ok(());
            }
            ev = rx.recv() => {
                let ev = match ev {
                    Ok(ev) if ev.job_id() == job_id => ev,
                    Ok(_) | Err(RecvError::Lagged(_)) => continue,
                    Err(RecvError::Closed) => continue,
                };
                let line = match ev {
                    Event::TransferProgress { bytes_done, bytes_total, files_done, files_total, bytes_per_sec, eta_secs, .. } => {
                        if last_line.elapsed() < Duration::from_millis(500) { continue; }
                        last_line = Instant::now();
                        let pct = if bytes_total > 0 { bytes_done as f64 * 100.0 / bytes_total as f64 } else { 100.0 };
                        let eta = eta_secs.map(|s| format!(", {} left", human_secs(s))).unwrap_or_default();
                        Some((true, format!("{pct:5.1}%  {files_done}/{files_total} files  {} / {}  {}/s{eta}",
                            human_bytes(bytes_done), human_bytes(bytes_total), human_bytes(bytes_per_sec as u64))))
                    }
                    Event::VerifyProgress { checked, total, mismatches, .. } => {
                        if last_line.elapsed() < Duration::from_millis(500) { continue; }
                        last_line = Instant::now();
                        Some((true, format!("verifying {checked}/{total}, {mismatches} differ")))
                    }
                    Event::IntegrationProgress { step, done, total, items, waiting, .. } => {
                        let w = waiting.map(|w| format!(" ({w})")).unwrap_or_default();
                        Some((true, format!("{step}: {done}/{total} folders, {items} items{w}")))
                    }
                    Event::JobStateChanged { to, error, .. } => Some((false, format!("→ {}{}", to.as_str(),
                        error.map(|e| format!(": {e}")).unwrap_or_default()))),
                    Event::RetryScheduled { attempt, in_secs, error, .. } =>
                        Some((false, format!("! {error}; retry #{attempt} in {}", human_secs(in_secs)))),
                    Event::SendWindowWait { opens_at, .. } => {
                        let secs = (opens_at - filebridge_store::now_ms()).max(0) as u64 / 1000;
                        Some((false, format!("waiting for the send window; it opens in {} \
                            (`filebridge settings`)", human_secs(secs))))
                    }
                    Event::DestinationSpace { free_bytes, left_bytes, .. }
                        if free_bytes < left_bytes && !warned_space =>
                    {
                        warned_space = true;
                        Some((false, format!("! the destination may run out of space: {} free, {} still to send",
                            human_bytes(free_bytes), human_bytes(left_bytes))))
                    }
                    Event::EntryFailed { path, error, retryable: false, .. } =>
                        Some((false, format!("! {path}: {error}"))),
                    Event::IntegrationProblem { code, message, .. } =>
                        Some((false, format!("! {code}: {message}"))),
                    _ => None,
                };
                if let Some((progress, text)) = line {
                    if progress && tty {
                        eprint!("\r\x1b[2K{text}");
                        wrote_progress = true;
                    } else {
                        if wrote_progress && tty {
                            eprintln!();
                            wrote_progress = false;
                        }
                        eprintln!("{text}");
                    }
                    let _ = std::io::stderr().flush();
                }
            }
        }
    }
}

pub(crate) fn confirm(question: &str) -> Result<bool> {
    if !std::io::stdin().is_terminal() {
        bail!("not a terminal; pass --yes to send without asking");
    }
    eprint!("{question} [y/N] ");
    std::io::stderr().flush()?;
    let mut answer = String::new();
    std::io::stdin().read_line(&mut answer)?;
    Ok(matches!(answer.trim(), "y" | "Y" | "yes" | "s" | "sim"))
}

pub fn human_bytes(b: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = b as f64;
    let mut u = 0;
    while v >= 1000.0 && u < UNITS.len() - 1 {
        v /= 1000.0;
        u += 1;
    }
    if u == 0 {
        format!("{b} B")
    } else {
        format!("{v:.1} {}", UNITS[u])
    }
}

fn human_secs(s: u64) -> String {
    match s {
        0..60 => format!("{s}s"),
        60..3600 => format!("{}m{:02}s", s / 60, s % 60),
        _ => format!("{}h{:02}m", s / 3600, (s % 3600) / 60),
    }
}

fn hex(b: &[u8]) -> String {
    b.iter()
        .map(|x| format!("{x:02x}"))
        .collect::<Vec<_>>()
        .join(" ")
}

/// `filebridge report`: a finished job's report, to stdout or a file (docs/design/specs/D09).
pub async fn report(
    g: &Global,
    job: &str,
    format: crate::ReportFormatArg,
    only: Option<crate::ReportOnly>,
    dirs: bool,
    output: Option<PathBuf>,
) -> Result<()> {
    use filebridge_core::{ReportFormat, ReportRequest, ReportScope};
    let app = App::open(g, Access::Read, false).await?;
    let j = app.job(job).await?;
    let format = match format {
        crate::ReportFormatArg::Csv => ReportFormat::Csv,
        crate::ReportFormatArg::Json => ReportFormat::Json,
    };
    let scope = match only {
        None => ReportScope::All,
        Some(crate::ReportOnly::Problems) => ReportScope::Problems,
        Some(crate::ReportOnly::Renamed) => ReportScope::Renamed,
    };
    match output {
        Some(path) => {
            let r = app
                .engine
                .export_report(
                    &j.id,
                    ReportRequest {
                        format,
                        scope,
                        include_dirs: dirs,
                        path: path.display().to_string(),
                    },
                )
                .await?;
            eprintln!("{} rows, {} bytes → {}", r.rows, r.bytes, r.path);
        }
        None => {
            // `Stdout` (not its lock) is `Send`; buffer it for speed.
            let mut out = std::io::BufWriter::new(std::io::stdout());
            app.engine
                .write_report(&j.id, format, scope, dirs, &mut out)
                .await?;
        }
    }
    Ok(())
}
