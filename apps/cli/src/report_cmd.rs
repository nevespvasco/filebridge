//! `filebridge report`: a finished job's report (docs/design/specs/D09), `report diff` to compare
//! two, and `report check` for a sealed one (ADR 0025).

use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Result, bail};
use filebridge_core::report::{diff, seal};
use filebridge_core::{
    ReportChange, ReportChangeKind, ReportCheck, ReportDiff, ReportFormat, ReportRequest,
    ReportScope, SealProblem,
};
use filebridge_host::Access;

use crate::app::{App, Global};

#[derive(clap::Args)]
#[command(args_conflicts_with_subcommands = true, subcommand_negates_reqs = true)]
pub struct ReportArgs {
    #[command(subcommand)]
    cmd: Option<ReportCmd>,
    /// The job (id or unique id prefix).
    #[arg(required = true)]
    job: Option<String>,
    #[arg(long, value_enum, default_value_t = FormatArg::Csv)]
    format: FormatArg,
    /// Only the problems, or only the renamed files.
    #[arg(long, value_enum)]
    only: Option<OnlyArg>,
    /// Folders too.
    #[arg(long)]
    dirs: bool,
    /// Write to this file instead of stdout (`sealed` needs one: FILE.json and FILE.html).
    #[arg(short, long)]
    output: Option<PathBuf>,
}

#[derive(clap::Subcommand)]
enum ReportCmd {
    /// What changed from one finished job's report to another's, files matched by source path:
    /// one `kind<TAB>source path<TAB>detail` line per change. Each side is a job (id or prefix)
    /// or an exported JSON report. Exits 1 when there are changes, like `diff`.
    Diff {
        a: String,
        b: String,
        /// Only this kind of change.
        #[arg(long, value_enum)]
        only: Option<ChangeArg>,
    },
    /// Whether a sealed report (its .json or its .html copy; the other one beside it too) is
    /// exactly as exported. Exits 1 when it isn't.
    Check { file: PathBuf },
}

#[derive(Debug, Clone, Copy, clap::ValueEnum)]
enum FormatArg {
    Csv,
    Json,
    /// JSON with a digest chain and a final SHA-256, plus a readable HTML copy.
    Sealed,
}

#[derive(Debug, Clone, Copy, clap::ValueEnum)]
enum OnlyArg {
    Problems,
    Renamed,
}

#[derive(Debug, Clone, Copy, clap::ValueEnum)]
enum ChangeArg {
    Added,
    Removed,
    Changed,
    Renamed,
    NewlyVerified,
    NewProblem,
}

impl From<ChangeArg> for ReportChangeKind {
    fn from(k: ChangeArg) -> Self {
        match k {
            ChangeArg::Added => Self::Added,
            ChangeArg::Removed => Self::Removed,
            ChangeArg::Changed => Self::Changed,
            ChangeArg::Renamed => Self::Renamed,
            ChangeArg::NewlyVerified => Self::NewlyVerified,
            ChangeArg::NewProblem => Self::NewProblem,
        }
    }
}

pub async fn run(g: &Global, args: ReportArgs) -> Result<()> {
    match args.cmd {
        Some(ReportCmd::Diff { a, b, only }) => diff_cmd(g, &a, &b, only.map(Into::into)).await,
        Some(ReportCmd::Check { file }) => check_cmd(&file),
        None => {
            let job = args.job.expect("clap requires the job");
            export(g, &job, args.format, args.only, args.dirs, args.output).await
        }
    }
}

async fn export(
    g: &Global,
    job: &str,
    format: FormatArg,
    only: Option<OnlyArg>,
    dirs: bool,
    output: Option<PathBuf>,
) -> Result<()> {
    let app = App::open(g, Access::Read, false).await?;
    let j = app.job(job).await?;
    let format = match format {
        FormatArg::Csv => ReportFormat::Csv,
        FormatArg::Json => ReportFormat::Json,
        FormatArg::Sealed => ReportFormat::Sealed,
    };
    let scope = match only {
        None => ReportScope::All,
        Some(OnlyArg::Problems) => ReportScope::Problems,
        Some(OnlyArg::Renamed) => ReportScope::Renamed,
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
            if let (Some(html), Some(seal)) = (r.readable, r.seal) {
                eprintln!("readable copy → {html}");
                eprintln!("seal SHA-256 {seal} (keep it somewhere else: it proves both unchanged)");
            }
        }
        None if format == ReportFormat::Sealed => {
            bail!("a sealed report is two files: give one with -o FILE.json")
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

async fn diff_cmd(g: &Global, a: &str, b: &str, only: Option<ReportChangeKind>) -> Result<()> {
    let mut out = std::io::BufWriter::new(std::io::stdout());
    let mut failed = None;
    let mut print = |c: ReportChange| {
        if failed.is_none()
            && let Err(e) = print_change(&mut out, &c, only)
        {
            failed = Some(e);
        }
    };
    let (pa, pb) = (Path::new(a), Path::new(b));
    let d = match (pa.is_file(), pb.is_file()) {
        (true, true) => {
            let (ra, rb) = (diff::read_report(pa)?, diff::read_report(pb)?);
            eprintln!("A: {}", file_title(pa, &ra.transfer));
            eprintln!("B: {}", file_title(pb, &rb.transfer));
            diff::diff_rows(ra.entries, rb.entries, &mut print)
        }
        (false, false) => {
            let app = App::open(g, Access::Read, false).await?;
            let (ja, jb) = (app.job(a).await?, app.job(b).await?);
            for (side, j) in [("A", &ja), ("B", &jb)] {
                let label = j.label.clone().unwrap_or_else(|| j.sources.join(", "));
                eprintln!("{side}: {}  {label}", &j.id[..8]);
            }
            let d = app
                .engine
                .diff_reports_with(&ja.id, &jb.id, &mut print)
                .await?;
            if !d.same_sources {
                eprintln!("note: the jobs have different sources; files are matched by path");
            }
            d
        }
        _ => bail!("compare two jobs or two exported JSON reports"),
    };
    if let Some(e) = failed {
        return Err(e.into());
    }
    out.flush()?;
    eprintln!("{}", summary(&d));
    let changed = d
        .counts
        .iter()
        .any(|c| c.files > 0 && only.is_none_or(|k| k == c.kind));
    if changed {
        std::process::exit(1);
    }
    Ok(())
}

fn file_title(path: &Path, transfer: &serde_json::Value) -> String {
    let label = transfer["label"].as_str().unwrap_or("");
    format!(
        "{}  {}{}",
        path.display(),
        transfer["id"].as_str().unwrap_or("?"),
        if label.is_empty() {
            String::new()
        } else {
            format!("  {label}")
        }
    )
}

fn print_change(
    out: &mut impl Write,
    c: &ReportChange,
    only: Option<ReportChangeKind>,
) -> std::io::Result<()> {
    for k in &c.kinds {
        if only.is_some_and(|o| o != *k) {
            continue;
        }
        let (a, b) = (c.a.as_ref(), c.b.as_ref());
        let detail = match k {
            ReportChangeKind::Added | ReportChangeKind::Removed => String::new(),
            ReportChangeKind::Changed => {
                let (a, b) = (a.expect("in both"), b.expect("in both"));
                if a.size != b.size {
                    format!("size {} → {}", a.size, b.size)
                } else {
                    format!(
                        "sha256 {} → {}",
                        a.sha256.as_deref().unwrap_or("-"),
                        b.sha256.as_deref().unwrap_or("-")
                    )
                }
            }
            ReportChangeKind::Renamed => {
                let b = b.expect("in both");
                let rules = if b.renamed_by.is_empty() {
                    String::new()
                } else {
                    format!(" ({})", b.renamed_by.join(", "))
                };
                format!(
                    "{} → {}{rules}",
                    a.and_then(|a| a.destination_path.as_deref()).unwrap_or(""),
                    b.destination_path.as_deref().unwrap_or("")
                )
            }
            ReportChangeKind::NewlyVerified | ReportChangeKind::NewProblem => format!(
                "{} → {}",
                a.map(|a| a.status.as_str()).unwrap_or("-"),
                b.map(|b| b.status.as_str()).unwrap_or("-")
            ),
        };
        writeln!(out, "{}\t{}\t{detail}", k.as_str(), c.source_path)?;
    }
    Ok(())
}

fn summary(d: &ReportDiff) -> String {
    let mut parts: Vec<String> = d
        .counts
        .iter()
        .map(|c| format!("{} {}", c.files, c.kind.as_str().replace('_', " ")))
        .collect();
    parts.push(format!("{} unchanged", d.unchanged));
    parts.join(" · ")
}

fn check_cmd(file: &Path) -> Result<()> {
    let c = seal::check_file(file)?;
    print_check(&c);
    if !c.intact {
        std::process::exit(1);
    }
    Ok(())
}

fn print_check(c: &ReportCheck) {
    let copy = c
        .readable
        .as_deref()
        .map(|r| format!(" and {r}"))
        .unwrap_or_default();
    if c.intact {
        println!(
            "intact: {}{copy}, {} entries, as exported",
            c.path, c.entries
        );
        println!("seal SHA-256 {}", c.sha256.as_deref().unwrap_or_default());
        if c.readable.is_none() {
            println!("(no readable .html copy beside it)");
        }
        return;
    }
    println!("CHANGED: {}{copy}", c.path);
    for p in &c.problems {
        match p {
            SealProblem::Unreadable { detail } => {
                println!("  not readable as a filebridge report: {detail}")
            }
            SealProblem::NotSealed => {
                println!("  exported without a seal: nothing to check (export it as `sealed`)")
            }
            SealProblem::Header => println!("  the summary at the top was changed"),
            SealProblem::Entry { index, source_path } => println!(
                "  entry {} ({source_path}) was changed, or one just before it removed or added",
                index + 1
            ),
            SealProblem::Seal => {
                println!("  entries removed or added at the end, or the seal itself was changed")
            }
            SealProblem::Readable => println!("  the readable copy doesn't match the JSON"),
        }
    }
    if c.more > 0 {
        println!("  …and {} more entries", c.more);
    }
    if let Some(sha) = &c.sha256 {
        println!("seal SHA-256 recorded in the file: {sha}");
    }
}
