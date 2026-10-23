//! Scheduled sends (docs/adr/0019): a saved send that runs on its own at set times, in the
//! service, and sends only what changed since its last run. A watched schedule (docs/adr/0023)
//! runs when its sources change instead.

use serde::{Deserialize, Serialize};

use crate::{JobState, SendOptions};

/// The most characters in a schedule's name.
pub const MAX_SCHEDULE_NAME: usize = 120;

/// How long a watched schedule waits, by default, for its sources to stop changing.
pub const DEFAULT_QUIET_SECS: u32 = 60;
/// The bounds of a watched schedule's quiet time.
pub const MIN_QUIET_SECS: u32 = 5;
pub const MAX_QUIET_SECS: u32 = 3600;

/// A day of the week.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, specta::Type,
)]
#[serde(rename_all = "snake_case")]
pub enum Weekday {
    Mon,
    Tue,
    Wed,
    Thu,
    Fri,
    Sat,
    Sun,
}

impl Weekday {
    pub const ALL: [Weekday; 7] = [
        Weekday::Mon,
        Weekday::Tue,
        Weekday::Wed,
        Weekday::Thu,
        Weekday::Fri,
        Weekday::Sat,
        Weekday::Sun,
    ];

    /// Days since Monday (0–6).
    pub fn index(self) -> u32 {
        self as u32
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Weekday::Mon => "mon",
            Weekday::Tue => "tue",
            Weekday::Wed => "wed",
            Weekday::Thu => "thu",
            Weekday::Fri => "fri",
            Weekday::Sat => "sat",
            Weekday::Sun => "sun",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|d| d.as_str().eq_ignore_ascii_case(s.trim()))
    }
}

/// When a schedule runs, in the service machine's local time.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Recurrence {
    /// On each of `days` at `minute` past midnight.
    Days { days: Vec<Weekday>, minute: u16 },
    /// Every day at `minute` past midnight, then every `every` hours until midnight.
    Hours { every: u16, minute: u16 },
    /// When its sources change, once they have been quiet for `quiet_secs` (docs/adr/0023).
    OnChange { quiet_secs: u32 },
}

impl Recurrence {
    /// Why this recurrence can't be saved, if it can't.
    pub fn validate(&self) -> Result<(), String> {
        let minute = match self {
            Recurrence::OnChange { quiet_secs } => {
                if !(MIN_QUIET_SECS..=MAX_QUIET_SECS).contains(quiet_secs) {
                    return Err(format!(
                        "the quiet time must be from {MIN_QUIET_SECS} s to {} min",
                        MAX_QUIET_SECS / 60
                    ));
                }
                return Ok(());
            }
            Recurrence::Days { days, minute } => {
                if days.is_empty() {
                    return Err("pick at least one day".into());
                }
                *minute
            }
            Recurrence::Hours { every, minute } => {
                if !(1..=24).contains(every) {
                    return Err("the hours between runs must be from 1 to 24".into());
                }
                *minute
            }
        };
        if minute >= 24 * 60 {
            return Err("a time must be between 00:00 and 23:59".into());
        }
        Ok(())
    }

    /// The minutes past midnight it runs at on a day it runs, in order (none when it runs on
    /// change).
    pub fn minutes_of_day(&self) -> Vec<u16> {
        match self {
            Recurrence::OnChange { .. } => Vec::new(),
            Recurrence::Days { minute, .. } => vec![*minute],
            Recurrence::Hours { every, minute } => {
                let step = usize::from((*every).max(1)) * 60;
                (usize::from(*minute)..24 * 60)
                    .step_by(step)
                    .map(|m| m as u16)
                    .collect()
            }
        }
    }

    /// Whether it runs at all on `day`.
    pub fn runs_on(&self, day: Weekday) -> bool {
        match self {
            Recurrence::Days { days, .. } => days.contains(&day),
            Recurrence::Hours { .. } => true,
            Recurrence::OnChange { .. } => false,
        }
    }

    /// The quiet time of a watched schedule; `None` for a timetable.
    pub fn quiet_secs(&self) -> Option<u32> {
        match self {
            Recurrence::OnChange { quiet_secs } => Some(*quiet_secs),
            _ => None,
        }
    }
}

/// How a watched schedule's sources are being watched (docs/adr/0023).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum WatchMode {
    /// The operating system reports changes as they happen.
    Events,
    /// It can't (a watch limit, a folder that is gone): the folder is scanned every few minutes
    /// instead; `WatchView::reason` says why.
    Scanning,
    /// This engine isn't watching it: switched off, or an engine that runs no schedules (the
    /// CLI's own).
    Idle,
}

/// A watched schedule's watch, as the window shows it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type)]
pub struct WatchView {
    pub mode: WatchMode,
    /// Why it is scanning instead of watching.
    pub reason: Option<String>,
    /// Whether `reason` is the system's limit on watched folders (Linux inotify), which the
    /// user can raise.
    pub limit: bool,
    /// The last change seen in its sources (Unix ms).
    pub last_change_at: Option<i64>,
    /// The first change not in a run yet (Unix ms); `None` when everything seen has gone.
    pub pending_since: Option<i64>,
}

/// A schedule as the window or the CLI saves it. Without `id` it is new; with one, it replaces
/// that schedule's name, recurrence, `verify` and `enabled` (its sources, destination and target
/// folder are fixed once saved).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type)]
pub struct ScheduleInput {
    pub id: Option<String>,
    pub name: String,
    /// Absolute paths of the files and folders it sends.
    pub sources: Vec<String>,
    pub profile_id: String,
    pub options: SendOptions,
    pub recurrence: Recurrence,
    pub enabled: bool,
}

/// What happened the last time a schedule's time came.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum ScheduleOutcome {
    /// A run was created; its job is `last_job_id`.
    Started,
    /// Nothing was new or changed: no run was kept.
    NothingChanged,
    /// The previous run was not finished: no new one started.
    StillRunning,
    /// The run could not be created or analysed; `last_error` says why.
    Failed,
}

impl ScheduleOutcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Started => "started",
            Self::NothingChanged => "nothing_changed",
            Self::StillRunning => "still_running",
            Self::Failed => "failed",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "started" => Self::Started,
            "nothing_changed" => Self::NothingChanged,
            "still_running" => Self::StillRunning,
            "failed" => Self::Failed,
            _ => return None,
        })
    }
}

/// A schedule as the window shows it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type)]
pub struct ScheduleView {
    pub id: String,
    pub name: String,
    /// Source roots, lossily decoded for display.
    pub sources: Vec<String>,
    pub profile_id: String,
    pub options: SendOptions,
    pub recurrence: Recurrence,
    pub enabled: bool,
    /// When it runs next (Unix ms); `None` while disabled.
    pub next_run_at: Option<i64>,
    /// When its time last came (Unix ms), whatever happened then.
    pub last_run_at: Option<i64>,
    pub last_outcome: Option<ScheduleOutcome>,
    pub last_error: Option<String>,
    /// Its newest run, and that job's state.
    pub last_job_id: Option<String>,
    pub last_job_state: Option<JobState>,
    /// Files it has sent so far (one per source file, whatever the number of runs).
    pub files_sent: u64,
    /// For a watched schedule (`Recurrence::OnChange`): its watch.
    pub watch: Option<WatchView>,
    pub created_at: i64,
    pub updated_at: i64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hours_from_a_minute_until_midnight() {
        let r = Recurrence::Hours {
            every: 6,
            minute: 30,
        };
        assert_eq!(r.minutes_of_day(), [30, 390, 750, 1110]);
        let r = Recurrence::Hours {
            every: 5,
            minute: 0,
        };
        assert_eq!(r.minutes_of_day(), [0, 300, 600, 900, 1200]);
        assert!(r.runs_on(Weekday::Sun));
    }

    #[test]
    fn validation() {
        let days = |d: Vec<Weekday>, m| Recurrence::Days { days: d, minute: m };
        assert!(days(vec![Weekday::Mon], 0).validate().is_ok());
        assert!(days(vec![], 0).validate().is_err());
        assert!(days(vec![Weekday::Mon], 1440).validate().is_err());
        let hours = |every, minute| Recurrence::Hours { every, minute };
        assert!(hours(24, 1439).validate().is_ok());
        assert!(hours(0, 0).validate().is_err());
        assert!(hours(25, 0).validate().is_err());
        let watch = |quiet_secs| Recurrence::OnChange { quiet_secs };
        assert!(watch(DEFAULT_QUIET_SECS).validate().is_ok());
        assert!(watch(MIN_QUIET_SECS - 1).validate().is_err());
        assert!(watch(MAX_QUIET_SECS + 1).validate().is_err());
        assert!(watch(60).minutes_of_day().is_empty());
        assert!(!watch(60).runs_on(Weekday::Mon));
        assert_eq!(watch(60).quiet_secs(), Some(60));
    }

    #[test]
    fn weekday_names() {
        for d in Weekday::ALL {
            assert_eq!(Weekday::parse(d.as_str()), Some(d));
        }
        assert_eq!(Weekday::parse("FRI"), Some(Weekday::Fri));
        assert_eq!(Weekday::parse("x"), None);
        assert_eq!(Weekday::Sun.index(), 6);
    }
}
