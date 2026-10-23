//! Phase 0 benchmark: baseline `sync/copy` vs plan A on the same source, plus plan A sent the
//! way the app sends it (`engine`: the Engine with its SQLite manifest, batches and
//! per-file state saving), so the gate measures the pipeline that ships.
//!
//! Each mode writes to its own new folder `<dest-root>/filebridge-bench-<unix time>/<mode>`.
//! The source is only ever read. Nothing at the destination is deleted.

use std::collections::BTreeSet;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use filebridge_core::{
    DecisionInput, Engine, EngineOptions, Event, JobState, Profile, Registry, SendOptions, Store,
    TargetSpec,
};
use filebridge_store::EntryState;
use filebridge_transport_api::{
    Backend, Destination, ProgressEvent, ProgressSink, StatsSnapshot, TransferJob, TransferOptions,
    Transport,
};
use filebridge_transport_rclone::{RcClient, Rcd, RcloneTransport, fs, stats};
use serde::Serialize;
use serde_json::json;

use crate::prepare::{Prepared, prepare};

#[derive(clap::Args)]
pub struct Args {
    /// Local folder to send (read-only).
    #[arg(long)]
    source: PathBuf,
    /// Folder on the destination under which the bench folder is created.
    #[arg(long)]
    dest_root: String,
    /// SFTP host; without it the destination is a local folder.
    #[arg(long)]
    sftp_host: Option<String>,
    #[arg(long)]
    sftp_port: Option<u16>,
    #[arg(long)]
    sftp_user: Option<String>,
    /// Private key; without it the SSH agent is used.
    #[arg(long)]
    key_file: Option<PathBuf>,
    /// Defaults to ~/.ssh/known_hosts. Host keys are always verified.
    #[arg(long)]
    known_hosts: Option<PathBuf>,
    #[arg(
        long,
        value_enum,
        value_delimiter = ',',
        default_value = "baseline,plan-a,engine"
    )]
    modes: Vec<Mode>,
    /// Files in flight (rclone --transfers for sync/copy, parallel copyfile calls for plan A).
    #[arg(long, default_value_t = 8)]
    concurrency: usize,
    /// Write results as JSON here (its folder is created first).
    #[arg(long)]
    json: Option<PathBuf>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum, Serialize)]
#[serde(rename_all = "kebab-case")]
enum Mode {
    /// Plain sync/copy, original names: the speed to beat.
    Baseline,
    /// operations/copyfile per file, straight to the NFC name.
    PlanA,
    /// Plan A through the Engine, as the app sends: analysis, SQLite manifest, batches.
    Engine,
}

impl Mode {
    fn dir(self) -> &'static str {
        match self {
            Mode::Baseline => "baseline",
            Mode::PlanA => "plan-a",
            Mode::Engine => "engine",
        }
    }
}

#[derive(Serialize)]
struct Run {
    mode: Mode,
    concurrency: usize,
    files: usize,
    bytes: u64,
    seconds: f64,
    files_per_sec: f64,
    mb_per_sec: f64,
    stages: Vec<(String, f64)>,
    failed: usize,
    /// Files left out because their name needed a decision (`engine` only): not sent, and not
    /// counted in `files`.
    skipped: usize,
    /// Expected destination paths not found after the run.
    missing: usize,
    /// Destination paths that were not expected (e.g. a name left uncorrected).
    unexpected: usize,
}

/// `rclone`: the global `--rclone` (or `FILEBRIDGE_RCLONE`); found like the engine commands'
/// when absent.
pub async fn run(args: Args, rclone: Option<&Path>) -> Result<()> {
    // The JSON is written at the end: a missing folder must not lose a long run.
    if let Some(dir) = args
        .json
        .as_deref()
        .and_then(Path::parent)
        .filter(|d| !d.as_os_str().is_empty())
    {
        std::fs::create_dir_all(dir)
            .with_context(|| format!("create {} for --json", dir.display()))?;
    }
    let prepared = prepare(&args.source)?;
    let (rf, rd) = prepared.renamed_nodes();
    eprintln!(
        "source: {} files, {} dirs, {:.1} MB; nfc corrects {rf} file and {rd} dir names ({} file paths change); {} non-UTF-8 skipped",
        prepared.files(),
        prepared.dirs(),
        prepared.scan.total_bytes() as f64 / 1e6,
        prepared.renamed_files,
        prepared.unmappable.len(),
    );
    if args.concurrency == 0 {
        bail!("--concurrency must be at least 1");
    }

    let rcd = Arc::new(Rcd::start(&filebridge_host::find_rclone(rclone)?).await?);
    let rc = RcClient::from(rcd.clone());
    let version = rc.call("core/version", json!({})).await?;
    let version = version["version"].as_str().unwrap_or("?").to_owned();
    let stamp = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
    let backend = match &args.sftp_host {
        Some(host) => Backend::Sftp {
            host: host.clone(),
            port: args.sftp_port,
            user: args.sftp_user.clone(),
            key_file: args.key_file.clone(),
            known_hosts_file: args.known_hosts.clone(),
            password: None,
            key: None,
        },
        None => Backend::Local,
    };
    let bench_root = fs::join(&args.dest_root, &format!("filebridge-bench-{stamp}"));
    let bench = Destination {
        transport: RcloneTransport::ID.into(),
        backend,
        root: bench_root.clone(),
    };
    // Creates the bench folder and warms up the connection, so the first mode isn't penalised.
    rc.call(
        "operations/mkdir",
        json!({ "fs": fs::dest_fs(&bench, &fs::Creds::default(), "")?, "remote": "" }),
    )
    .await
    .context("cannot create the bench folder at the destination")?;
    eprintln!("rclone {version}; writing under {bench_root}");

    let mut runs = Vec::new();
    for mode in &args.modes {
        let dest = Destination {
            root: fs::join(&bench_root, mode.dir()),
            ..bench.clone()
        };
        let run = run_mode(*mode, &rcd, &dest, &prepared, &args, stamp).await?;
        runs.push(run);
    }

    println!(
        "\nmode      files     MB        s   files/s    MB/s  failed missing unexpected skipped"
    );
    for r in &runs {
        println!(
            "{:<8} {:>6} {:>8.1} {:>8.1} {:>9.1} {:>7.1} {:>7} {:>7} {:>10} {:>7}",
            r.mode.dir(),
            r.files,
            r.bytes as f64 / 1e6,
            r.seconds,
            r.files_per_sec,
            r.mb_per_sec,
            r.failed,
            r.missing,
            r.unexpected,
            r.skipped
        );
        for (stage, secs) in &r.stages {
            println!("           {stage}: {secs:.1} s");
        }
    }
    if let Some(path) = &args.json {
        let out = json!({
            "rclone": version,
            "destination": if args.sftp_host.is_some() { "sftp" } else { "local" },
            "started_at_unix": stamp,
            "source": {
                "files": prepared.files(),
                "dirs": prepared.dirs(),
                "bytes": prepared.scan.total_bytes(),
                "nfc_renamed_files": rf,
                "nfc_renamed_dirs": rd,
                "file_paths_changed": prepared.renamed_files,
                "non_utf8_skipped": prepared.unmappable.len(),
            },
            "runs": runs,
        });
        std::fs::write(path, serde_json::to_string_pretty(&out)? + "\n")?;
        eprintln!("results written to {}", path.display());
    }
    eprintln!("bench folders were left at the destination under {bench_root}");
    Ok(())
}

async fn run_mode(
    mode: Mode,
    rcd: &Arc<Rcd>,
    dest: &Destination,
    p: &Prepared,
    args: &Args,
    stamp: u64,
) -> Result<Run> {
    let rc = &RcClient::from(rcd.clone());
    let (sink, rx) = ProgressSink::channel();
    let printer = tokio::spawn(print_progress(mode, rx));
    let started = Instant::now();

    let (files, bytes, failed, skipped, expected) = match mode {
        Mode::Baseline => {
            let group = format!("bench-{stamp}-baseline");
            let totals = (p.files() as u64, p.scan.total_bytes());
            let poller = stats::spawn_poller(
                rc.clone(),
                group.clone(),
                Some(totals),
                None,
                sink.clone(),
                Duration::from_millis(500),
            );
            sink.send(ProgressEvent::Stage("copy".into()));
            let c = args.concurrency;
            let res = rc
                .call_job(
                    "sync/copy",
                    json!({
                        "srcFs": fs::local_fs(&args.source)?,
                        "dstFs": fs::dest_fs(dest, &fs::Creds::default(), "")?,
                        "_config": { "Transfers": c, "Checkers": c * 2 },
                        "_group": group,
                    }),
                )
                .await;
            poller.abort();
            res?;
            let expected = p
                .scan
                .files()
                .filter_map(|e| String::from_utf8(e.raw_path.clone()).ok())
                .collect::<BTreeSet<_>>();
            (totals.0 as usize, totals.1, 0, 0, expected)
        }
        Mode::PlanA => {
            let transport = RcloneTransport::new(rcd.clone());
            let job = TransferJob {
                id: format!("{stamp}-{}", mode.dir()),
                source_roots: vec![args.source.clone()],
                items: p.items.clone(),
                options: TransferOptions {
                    concurrency: args.concurrency,
                },
            };
            let res = transport.transfer(dest, &job, sink.clone()).await?;
            let expected = p.items.iter().map(|i| i.dest.as_str().to_owned()).collect();
            (p.items.len(), res.bytes, res.failed(), 0, expected)
        }
        Mode::Engine => through_engine(rcd, dest, args, stamp, &sink).await?,
    };
    let seconds = started.elapsed().as_secs_f64();
    drop(sink);
    if skipped > 0 {
        eprintln!(
            "  {skipped} files skipped: their names need a decision; not sent, not in files/s"
        );
    }
    let stage_starts = tokio::time::timeout(Duration::from_secs(5), printer)
        .await
        .ok()
        .and_then(|r| r.ok())
        .unwrap_or_default();

    let mut stages = Vec::new();
    for (i, (name, at)) in stage_starts.iter().enumerate() {
        let end = stage_starts.get(i + 1).map_or(seconds, |(_, next)| {
            next.duration_since(started).as_secs_f64()
        });
        stages.push((name.clone(), end - at.duration_since(started).as_secs_f64()));
    }

    let listed = rc
        .call(
            "operations/list",
            json!({ "fs": fs::dest_fs(dest, &fs::Creds::default(), "")?, "remote": "", "opt": { "recurse": true, "filesOnly": true } }),
        )
        .await?;
    let actual: BTreeSet<String> = listed["list"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|e| e["Path"].as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default();
    let missing = expected.difference(&actual).count();
    let unexpected = actual.difference(&expected).count();
    for path in expected.difference(&actual).take(3) {
        eprintln!("  missing: {path:?}");
    }
    for path in actual.difference(&expected).take(3) {
        eprintln!("  unexpected: {path:?}");
    }

    Ok(Run {
        mode,
        concurrency: args.concurrency,
        files,
        bytes,
        seconds,
        files_per_sec: files as f64 / seconds,
        mb_per_sec: bytes as f64 / 1e6 / seconds,
        stages,
        failed,
        skipped,
        missing,
        unexpected,
    })
}

/// Sends the source with a fresh Engine (its database in a private temp directory, removed on
/// every return path) and reports progress into `sink`. Name decisions that would block the send
/// are skipped. Returns the files sent, their bytes, failed and skipped files, and the expected
/// destination paths.
async fn through_engine(
    rcd: &Arc<Rcd>,
    dest: &Destination,
    args: &Args,
    stamp: u64,
    sink: &ProgressSink,
) -> Result<(usize, u64, usize, usize, BTreeSet<String>)> {
    let tmp = tempfile::Builder::new()
        .prefix(&format!("filebridge-bench-{stamp}-"))
        .tempdir()
        .context("create the bench database directory")?;
    let store = Store::open(&tmp.path().join("bench.db")).context("open the bench database")?;
    let mut registry = Registry::new();
    registry.add_transport(Arc::new(RcloneTransport::new(rcd.clone())));
    let engine = Engine::new(
        store.clone(),
        registry,
        EngineOptions {
            inhibit_sleep: false,
            ..EngineOptions::default()
        },
    );
    let profile = engine
        .save_profile(Profile {
            id: String::new(),
            name: "bench".into(),
            destination: dest.clone(),
            integration: None,
            integration_settings: serde_json::Value::Null,
            target: TargetSpec::default(),
        })
        .await?;
    let options = SendOptions {
        concurrency: args.concurrency as u32,
        ..SendOptions::default()
    };
    let job = engine
        .create_job(vec![args.source.clone()], &profile.id, options)
        .await?;
    sink.send(ProgressEvent::Stage("analyse".into()));
    if engine.analyse(&job).await?.state == JobState::AwaitingApproval {
        engine.decide_all(&job, None, DecisionInput::Skip).await?;
    }

    let mut events = engine.subscribe();
    let progress = sink.clone();
    let started = Instant::now();
    let forward = tokio::spawn(async move {
        while let Ok(event) = events.recv().await {
            if let Event::TransferProgress {
                bytes_done,
                bytes_total,
                files_done,
                files_total,
                bytes_per_sec,
                ..
            } = event
            {
                progress.send(ProgressEvent::Stats(StatsSnapshot {
                    bytes: bytes_done,
                    total_bytes: bytes_total,
                    files_done,
                    total_files: files_total,
                    bytes_per_sec,
                    elapsed_secs: started.elapsed().as_secs_f64(),
                    ..StatsSnapshot::default()
                }));
            }
        }
    });
    sink.send(ProgressEvent::Stage("send".into()));
    engine.start(&job).await?;
    let state = engine.wait(&job).await?;
    forward.abort();
    if !state.is_terminal() {
        bail!("the engine stopped in state {}", state.as_str());
    }

    let summary = engine.job(&job).await?;
    let mut expected = BTreeSet::new();
    let mut after = None;
    loop {
        let page = store.entries_page(&job, after, 5_000).await?;
        let Some(last) = page.last() else { break };
        after = Some(last.entry.idx);
        for e in page {
            if !e.entry.is_dir
                && e.transfer_state == EntryState::Done
                && let Some(path) = e.dest_path
            {
                expected.insert(path);
            }
        }
    }
    drop(engine);
    Ok((
        summary.counts.done as usize,
        summary.counts.bytes_done,
        summary.counts.failed as usize,
        summary.counts.skipped as usize,
        expected,
    ))
}

/// Prints a one-line progress display; returns when every sink is dropped, with stage starts.
async fn print_progress(
    mode: Mode,
    mut rx: tokio::sync::mpsc::UnboundedReceiver<ProgressEvent>,
) -> Vec<(String, Instant)> {
    let mut stages: Vec<(String, Instant)> = Vec::new();
    let mut err = std::io::stderr();
    while let Some(ev) = rx.recv().await {
        match ev {
            ProgressEvent::Stage(name) => {
                if !stages.is_empty() {
                    let _ = writeln!(err);
                }
                stages.push((name, Instant::now()));
            }
            ProgressEvent::Stats(s) => {
                let stage = stages.last().map_or("", |(n, _)| n.as_str());
                let _ = write!(
                    err,
                    "\r\x1b[2K{} {stage}: {}/{} files, {:.1}/{:.1} MB, {:.1} MB/s, {:.0} s",
                    mode.dir(),
                    s.files_done,
                    s.total_files,
                    s.bytes as f64 / 1e6,
                    s.total_bytes as f64 / 1e6,
                    s.bytes_per_sec / 1e6,
                    s.elapsed_secs
                );
                let _ = err.flush();
            }
            ProgressEvent::ItemDone { .. } => {}
        }
    }
    let _ = writeln!(err);
    stages
}
