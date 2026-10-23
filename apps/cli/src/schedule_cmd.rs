//! `filebridge schedule`: scheduled sends (docs/adr/0019). `filebridge-service` runs them at
//! their times; these commands manage them, and `run-now` runs one here and waits for it, like
//! `filebridge send`.

use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use filebridge_core::{
    Recurrence, ScheduleInput, ScheduleOutcome, ScheduleView, SendOptions, Weekday,
};
use filebridge_host::Access;
use filebridge_ipc::Operations;

use crate::app::{App, Global};
use crate::send_cmd::human_secs;

#[derive(clap::Subcommand)]
pub enum ScheduleCmd {
    /// Add a scheduled send. Each run sends only what is new or changed since the last one; a
    /// changed file's older version is kept at the destination under a dated name.
    Add(AddArgs),
    /// Every schedule, with its next and last run.
    List,
    /// Forget a schedule and what it sent. Its runs stay in `filebridge jobs`.
    Remove { schedule: String },
    /// Run a schedule now and wait for it (enabled or not); its next time stays.
    RunNow { schedule: String },
}

#[derive(clap::Args)]
pub struct AddArgs {
    /// Files and folders to send. Each one keeps its name at the destination.
    #[arg(required = true)]
    paths: Vec<PathBuf>,
    /// Destination profile (name or id).
    #[arg(long)]
    to: String,
    /// The schedule's name.
    #[arg(long)]
    name: String,
    /// The days it runs, e.g. `mon,wed,fri`, or `daily`. Needs --at.
    #[arg(long, conflicts_with = "every_hours")]
    days: Option<String>,
    /// Local time, HH:MM: when it runs on its days, or with --every-hours its first run each
    /// day (default 00:00).
    #[arg(long)]
    at: Option<String>,
    /// Runs every N hours (1–24), from --at until midnight, every day.
    #[arg(long)]
    every_hours: Option<u16>,
    /// A folder under the destination's root to send into.
    #[arg(long)]
    into: Option<String>,
    /// Compare SHA-256 of every file at the end of each run and resend any that differ.
    #[arg(long)]
    verify: bool,
    /// Save it switched off: it runs only with `run-now`.
    #[arg(long)]
    disabled: bool,
}

pub async fn run(g: &Global, cmd: ScheduleCmd) -> Result<()> {
    match cmd {
        ScheduleCmd::Add(args) => add(g, args).await,
        ScheduleCmd::List => list(g).await,
        ScheduleCmd::Remove { schedule } => {
            let app = App::open(g, Access::Exclusive, false).await?;
            let s = find(&app, &schedule).await?;
            app.ops.delete_schedule(s.id.clone()).await?;
            println!("removed {}; its runs stay in `filebridge jobs`", s.name);
            Ok(())
        }
        ScheduleCmd::RunNow { schedule } => {
            let app = App::open(g, Access::Exclusive, true).await?;
            let s = find(&app, &schedule).await?;
            match app.ops.run_schedule_now(s.id.clone()).await? {
                None => println!("nothing new or changed since the last run"),
                Some(job) => crate::send_cmd::follow(&app.engine, &job).await?,
            }
            Ok(())
        }
    }
}

async fn add(g: &Global, args: AddArgs) -> Result<()> {
    let recurrence = recurrence(&args)?;
    // The destination's plugins check the schedule's profile and folder: they need the transport.
    let app = App::open(g, Access::Exclusive, true).await?;
    let profile = app.profile(&args.to).await?;
    let saved = app
        .ops
        .save_schedule(ScheduleInput {
            id: None,
            name: args.name,
            sources: args
                .paths
                .iter()
                .map(|p| std::path::absolute(p).map(|p| p.to_string_lossy().into_owned()))
                .collect::<std::io::Result<_>>()?,
            profile_id: profile.id,
            options: SendOptions {
                verify: args.verify,
                target_dir: args.into,
                ..SendOptions::default()
            },
            recurrence,
            enabled: !args.disabled,
        })
        .await?;
    print(&saved, &profile.name);
    println!(
        "filebridge-service runs it; files changed at the source keep their older version at the \
         destination"
    );
    Ok(())
}

async fn list(g: &Global) -> Result<()> {
    let app = App::open(g, Access::Read, false).await?;
    let schedules = app.ops.list_schedules().await?;
    if schedules.is_empty() {
        println!("no schedules (add one with `filebridge schedule add`)");
    }
    let profiles = app.ops.list_profiles().await?;
    for s in &schedules {
        let to = profiles
            .iter()
            .find(|p| p.id == s.profile_id)
            .map_or("?", |p| p.name.as_str());
        print(s, to);
    }
    Ok(())
}

fn print(s: &ScheduleView, to: &str) {
    let now = filebridge_store::now_ms();
    let next = match s.next_run_at {
        Some(t) if s.enabled => format!("next in {}", human_secs(((t - now).max(0) / 1000) as u64)),
        _ => "off".into(),
    };
    println!(
        "{}  {}  {}, {}",
        &s.id[..8.min(s.id.len())],
        s.name,
        describe(&s.recurrence),
        next
    );
    println!("    {} → {to}", s.sources.join(", "));
    if let (Some(at), Some(outcome)) = (s.last_run_at, s.last_outcome) {
        let ago = human_secs(((now - at).max(0) / 1000) as u64);
        let what = match outcome {
            ScheduleOutcome::Started => format!(
                "ran (job {}{})",
                s.last_job_id
                    .as_deref()
                    .map_or("?", |j| &j[..8.min(j.len())]),
                s.last_job_state
                    .map(|st| format!(", {}", st.as_str()))
                    .unwrap_or_default()
            ),
            ScheduleOutcome::NothingChanged => "nothing new or changed".into(),
            ScheduleOutcome::StillRunning => "skipped: the last run wasn't finished".into(),
            ScheduleOutcome::Failed => {
                format!("failed: {}", s.last_error.as_deref().unwrap_or_default())
            }
        };
        println!(
            "    last: {ago} ago, {what}; {} files sent so far",
            s.files_sent
        );
    }
}

/// `mon, wed at 03:00`, `every 6 h from 00:30`.
fn describe(r: &Recurrence) -> String {
    match r {
        Recurrence::Days { days, minute } => {
            let days = if days.len() == 7 {
                "daily".to_owned()
            } else {
                days.iter()
                    .map(|d| d.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            };
            format!("{days} at {}", hm(*minute))
        }
        Recurrence::Hours { every, minute } => format!("every {every} h from {}", hm(*minute)),
    }
}

fn hm(minute: u16) -> String {
    format!("{:02}:{:02}", minute / 60, minute % 60)
}

/// A schedule by id, id prefix or exact name.
async fn find(app: &App, key: &str) -> Result<ScheduleView> {
    let all = app.ops.list_schedules().await?;
    if let Some(s) = all.iter().find(|s| s.id == key || s.name == key) {
        return Ok(s.clone());
    }
    let found: Vec<_> = all.iter().filter(|s| s.id.starts_with(key)).collect();
    match found.as_slice() {
        [one] => Ok((*one).clone()),
        [] => bail!("no schedule {key:?} (see `filebridge schedule list`)"),
        _ => bail!(
            "{} schedules start with {key:?}; give more characters",
            found.len()
        ),
    }
}

fn recurrence(args: &AddArgs) -> Result<Recurrence> {
    let at = args.at.as_deref().map(parse_hm).transpose()?;
    let r = match args.every_hours {
        Some(every) => Recurrence::Hours {
            every,
            minute: at.unwrap_or(0),
        },
        None => {
            let minute = at.context("say when it runs: --at HH:MM, or --every-hours N")?;
            let days = match args.days.as_deref().map(str::trim) {
                None | Some("daily") => Weekday::ALL.to_vec(),
                Some(list) => list
                    .split(',')
                    .map(|d| Weekday::parse(d).with_context(|| format!("not a day: {d:?}")))
                    .collect::<Result<_>>()?,
            };
            Recurrence::Days { days, minute }
        }
    };
    r.validate().map_err(anyhow::Error::msg)?;
    Ok(r)
}

/// `HH:MM` → minutes past midnight.
fn parse_hm(t: &str) -> Result<u16> {
    let (h, m) = t
        .trim()
        .split_once(':')
        .with_context(|| format!("not a time: {t:?} (use HH:MM)"))?;
    let (h, m): (u16, u16) = (h.parse()?, m.parse()?);
    if h > 23 || m > 59 {
        bail!("not a time: {t:?}");
    }
    Ok(h * 60 + m)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(days: Option<&str>, at: Option<&str>, every: Option<u16>) -> AddArgs {
        AddArgs {
            paths: vec![],
            to: String::new(),
            name: String::new(),
            days: days.map(Into::into),
            at: at.map(Into::into),
            every_hours: every,
            into: None,
            verify: false,
            disabled: false,
        }
    }

    #[test]
    fn recurrences_from_flags() {
        assert_eq!(
            recurrence(&args(Some("mon,FRI"), Some("22:30"), None)).unwrap(),
            Recurrence::Days {
                days: vec![Weekday::Mon, Weekday::Fri],
                minute: 22 * 60 + 30
            }
        );
        assert_eq!(
            recurrence(&args(None, Some("03:00"), None)).unwrap(),
            Recurrence::Days {
                days: Weekday::ALL.to_vec(),
                minute: 180
            }
        );
        assert_eq!(
            recurrence(&args(None, None, Some(6))).unwrap(),
            Recurrence::Hours {
                every: 6,
                minute: 0
            }
        );
        assert!(recurrence(&args(None, None, None)).is_err(), "no time");
        assert!(recurrence(&args(Some("someday"), Some("03:00"), None)).is_err());
        assert!(recurrence(&args(None, Some("25:00"), None)).is_err());
        assert!(recurrence(&args(None, None, Some(0))).is_err());
    }

    #[test]
    fn descriptions() {
        assert_eq!(
            describe(&Recurrence::Days {
                days: vec![Weekday::Mon, Weekday::Wed],
                minute: 180
            }),
            "mon, wed at 03:00"
        );
        assert_eq!(
            describe(&Recurrence::Hours {
                every: 6,
                minute: 30
            }),
            "every 6 h from 00:30"
        );
    }
}
