//! Scheduled sends (docs/adr/0019): saved sends that the service runs at set times. Each run is
//! an ordinary Transfer job with `schedule_id` set; its analysis leaves out what the schedule
//! sent before and hasn't changed, carries the schedule's name decisions and accepts the rules'
//! proposals, and its transfer keeps the destination's older versions (`keep_older`).
//!
//! The time is always an argument ([`Engine::tick_schedules`]) or comes from
//! [`EngineOptions::wall_clock`](crate::EngineOptions), so tests drive schedules with a fake
//! clock.

use std::collections::HashMap;
use std::fmt;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use chrono::{Datelike, FixedOffset, NaiveDateTime, TimeZone};
use filebridge_model::{MAX_SCHEDULE_NAME, Op};
use filebridge_names::Choice;
use filebridge_store::{Decision, NewJob, ScheduleRow, ScheduleRun, now_ms};

use crate::analyse::{SentBefore, os_bytes};
use crate::{
    CoreError, Engine, Event, JobKind, JobState, Recurrence, Result, ScheduleInput,
    ScheduleOutcome, ScheduleView, SendOptions, Weekday, decisions,
};

/// The longest the scheduler sleeps before looking at the clock again: the monotonic clock
/// stops while the computer sleeps, and wall-clock time can jump (DST, a clock change).
const RECHECK: Duration = Duration::from_secs(60);

/// The time schedules run by: the system clock in the local time zone, or (tests) a fixed
/// function in a fixed time zone.
#[derive(Clone, Default)]
pub struct WallClock {
    now: Option<Arc<dyn Fn() -> i64 + Send + Sync>>,
    offset: Option<FixedOffset>,
}

impl WallClock {
    /// `now` in Unix ms, in a time zone `offset_secs` east of UTC, without DST.
    pub fn fixed(now: impl Fn() -> i64 + Send + Sync + 'static, offset_secs: i32) -> Self {
        Self {
            now: Some(Arc::new(now)),
            offset: Some(FixedOffset::east_opt(offset_secs).expect("an offset within a day")),
        }
    }

    pub fn now_ms(&self) -> i64 {
        match &self.now {
            Some(f) => f(),
            None => now_ms(),
        }
    }

    /// The first time `recurrence` comes after `after` (Unix ms).
    pub fn next_run(&self, recurrence: &Recurrence, after: i64) -> Option<i64> {
        match &self.offset {
            Some(tz) => next_in(tz, recurrence, after),
            None => next_in(&chrono::Local, recurrence, after),
        }
    }

    /// `2026-10-23 0300`: a moment as kept versions' names carry it (no `:`, which some
    /// destinations refuse).
    pub fn stamp(&self, at: i64) -> String {
        fn fmt<Tz: TimeZone>(tz: &Tz, at: i64) -> String
        where
            Tz::Offset: fmt::Display,
        {
            tz.timestamp_millis_opt(at)
                .earliest()
                .map(|t| t.format("%Y-%m-%d %H%M").to_string())
                .unwrap_or_default()
        }
        match &self.offset {
            Some(tz) => fmt(tz, at),
            None => fmt(&chrono::Local, at),
        }
    }
}

impl fmt::Debug for WallClock {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.offset {
            Some(o) => write!(f, "WallClock(fixed, {o})"),
            None => f.write_str("WallClock(system)"),
        }
    }
}

/// The first time `recurrence` comes after `after`, in `tz`.
fn next_in<Tz: TimeZone>(tz: &Tz, recurrence: &Recurrence, after: i64) -> Option<i64> {
    let start = tz.timestamp_millis_opt(after).earliest()?.date_naive();
    let minutes = recurrence.minutes_of_day();
    for d in 0..=8 {
        let date = start.checked_add_days(chrono::Days::new(d))?;
        let day = Weekday::ALL[date.weekday().num_days_from_monday() as usize];
        if !recurrence.runs_on(day) {
            continue;
        }
        for &m in &minutes {
            let local = date.and_hms_opt(u32::from(m / 60), u32::from(m % 60), 0)?;
            if let Some(at) = instant(tz, local)
                && at > after
            {
                return Some(at);
            }
        }
    }
    None
}

/// When a local time happens: its first time if it happens twice (DST ends), the first minute
/// after it if it doesn't happen (DST starts).
fn instant<Tz: TimeZone>(tz: &Tz, local: NaiveDateTime) -> Option<i64> {
    let mut t = local;
    for _ in 0..=24 * 60 {
        if let Some(at) = tz.from_local_datetime(&t).earliest() {
            return Some(at.timestamp_millis());
        }
        t += chrono::Duration::minutes(1);
    }
    None
}

impl Engine {
    pub async fn list_schedules(&self) -> Result<Vec<ScheduleView>> {
        let mut out = Vec::new();
        for row in self.store.list_schedules().await? {
            out.push(self.schedule_view(row).await?);
        }
        Ok(out)
    }

    pub async fn get_schedule(&self, id: &str) -> Result<ScheduleView> {
        let row = self.store.get_schedule(id).await?;
        self.schedule_view(row).await
    }

    async fn schedule_view(&self, row: ScheduleRow) -> Result<ScheduleView> {
        let latest = self.store.latest_run(&row.id).await?;
        let files_sent = self.store.sent_file_count(&row.id).await?;
        let (last_job_id, last_job_state) = match latest {
            Some((id, state, _)) => (Some(id), JobState::parse(&state).ok()),
            None => (None, None),
        };
        Ok(ScheduleView {
            recurrence: serde_json::from_value(row.recurrence)?,
            options: serde_json::from_value(row.options).unwrap_or_default(),
            sources: row
                .sources
                .iter()
                .map(|s| String::from_utf8_lossy(s).into_owned())
                .collect(),
            last_outcome: row.last_outcome.as_deref().and_then(ScheduleOutcome::parse),
            id: row.id,
            name: row.name,
            profile_id: row.profile_id,
            enabled: row.enabled,
            next_run_at: row.next_run_at,
            last_run_at: row.last_run_at,
            last_error: row.last_error,
            last_job_id,
            last_job_state,
            files_sent,
            created_at: row.created_at,
            updated_at: row.updated_at,
        })
    }

    /// Creates a schedule (no `id`) or changes one. Its sources, destination and target folder
    /// are fixed once saved: what changed since the last run depends on them.
    pub async fn save_schedule(&self, input: ScheduleInput) -> Result<ScheduleView> {
        let name = input.name.trim().to_owned();
        if name.is_empty() {
            return Err(CoreError::Invalid("give the schedule a name".into()));
        }
        if name.chars().count() > MAX_SCHEDULE_NAME || name.contains(['\n', '\r']) {
            return Err(CoreError::Invalid(format!(
                "a name is one line of at most {MAX_SCHEDULE_NAME} characters"
            )));
        }
        input.recurrence.validate().map_err(CoreError::Invalid)?;
        let mut options = input.options;
        options.keep_older = true;
        let clock = &self.options.wall_clock;
        let next_run_at = if input.enabled {
            clock.next_run(&input.recurrence, clock.now_ms())
        } else {
            None
        };
        let roots = schedule_sources(&input.sources)?;
        let id = match input.id {
            None => {
                let profile = self.get_profile(&input.profile_id).await?;
                self.registry.check_profile(&profile)?;
                self.check_target_dir(&profile, options.target_dir.as_deref())?;
                let id = uuid::Uuid::new_v4().to_string();
                self.store
                    .insert_schedule(ScheduleRow {
                        id: id.clone(),
                        name,
                        profile_id: profile.id,
                        options: serde_json::to_value(&options)?,
                        recurrence: serde_json::to_value(&input.recurrence)?,
                        enabled: input.enabled,
                        next_run_at,
                        last_run_at: None,
                        last_outcome: None,
                        last_error: None,
                        created_at: 0,
                        updated_at: 0,
                        sources: roots,
                    })
                    .await?;
                id
            }
            Some(id) => {
                let old = self.store.get_schedule(&id).await?;
                let old_options: SendOptions =
                    serde_json::from_value(old.options.clone()).unwrap_or_default();
                if old.profile_id != input.profile_id
                    || old.sources != roots
                    || old_options.target_dir != options.target_dir
                {
                    return Err(CoreError::Invalid(
                        "a schedule's files, destination and folder can't change; remove it and \
                         add a new one"
                            .into(),
                    ));
                }
                self.store
                    .update_schedule(ScheduleRow {
                        name,
                        options: serde_json::to_value(&options)?,
                        recurrence: serde_json::to_value(&input.recurrence)?,
                        enabled: input.enabled,
                        next_run_at,
                        ..old
                    })
                    .await?;
                id
            }
        };
        self.schedules_changed.notify_one();
        self.get_schedule(&id).await
    }

    /// Forgets a schedule and what it sent. Its runs stay in the Transfers list, as ordinary
    /// jobs; one still running goes on.
    pub async fn delete_schedule(&self, id: &str) -> Result<()> {
        let _guard = self.schedule_lock.lock().await;
        self.store.delete_schedule(id).await?;
        self.schedules_changed.notify_one();
        Ok(())
    }

    /// Runs a schedule now, enabled or not; its next time stays. Returns the run's job, or
    /// `None` when nothing changed since its last run. Refused while a run is unfinished.
    pub async fn run_schedule_now(self: &Arc<Self>, id: &str) -> Result<Option<String>> {
        let _guard = self.schedule_lock.lock().await;
        let row = self.store.get_schedule(id).await?;
        if !self.store.unfinished_runs(id).await?.is_empty() {
            return Err(CoreError::Invalid(
                "the schedule's last run isn't finished yet".into(),
            ));
        }
        let now = self.options.wall_clock.now_ms();
        let next = row.next_run_at;
        let ran = self.new_run(&row).await;
        self.record_run(&row.id, now, next, &ran).await?;
        ran
    }

    /// Runs the schedules whose time has come by `now` (Unix ms), once each however many times
    /// were missed, and moves each to its next time after `now`. A schedule whose last run isn't
    /// finished skips this time. Before that, carries on runs left half-way: a Draft (its
    /// analysis was cut short) is analysed again, a Ready one is started. Returns the runs it
    /// created or carried on (a new one may be waiting for name decisions).
    pub async fn tick_schedules(self: &Arc<Self>, now: i64) -> Result<Vec<String>> {
        let _guard = self.schedule_lock.lock().await;
        let mut started = Vec::new();
        for row in self.store.list_schedules().await? {
            for (job_id, state) in self.store.unfinished_runs(&row.id).await? {
                let carried_on = match JobState::parse(&state)? {
                    JobState::Draft => self.analyse_and_start(&job_id).await.map(|_| ()),
                    JobState::Ready => self.start(&job_id).await,
                    _ => continue,
                };
                match carried_on {
                    Ok(()) => started.push(job_id),
                    Err(e) => tracing::warn!(job = job_id, "a scheduled run can't go on: {e}"),
                }
            }
        }
        for row in self.store.due_schedules(now).await? {
            let next = serde_json::from_value::<Recurrence>(row.recurrence.clone())
                .ok()
                .and_then(|r| self.options.wall_clock.next_run(&r, now));
            let ran = if self.store.unfinished_runs(&row.id).await?.is_empty() {
                self.new_run(&row).await
            } else {
                Err(StillRunning.into())
            };
            if let Ok(Some(job)) = &ran {
                started.push(job.clone());
            }
            self.record_run(&row.id, now, next, &ran).await?;
        }
        Ok(started)
    }

    /// The service's timer: runs [`Engine::tick_schedules`] when a schedule is due, at least
    /// every minute, and when a schedule is saved or a run becomes Ready. Never returns.
    pub async fn run_scheduler(self: Arc<Self>) {
        loop {
            let now = self.options.wall_clock.now_ms();
            if let Err(e) = self.tick_schedules(now).await {
                tracing::warn!("scheduled sends: {e}");
            }
            let next = self.store.next_schedule_time().await.ok().flatten();
            let now = self.options.wall_clock.now_ms();
            let wait = next
                .map_or(RECHECK, |t| {
                    Duration::from_millis((t - now).max(0) as u64).min(RECHECK)
                })
                .max(Duration::from_secs(1));
            tokio::select! {
                _ = tokio::time::sleep(wait) => {}
                _ = self.schedules_changed.notified() => {}
            }
        }
    }

    /// Saves what a due time (or Run now) did, and tells the window.
    async fn record_run(
        &self,
        schedule_id: &str,
        at: i64,
        next_run_at: Option<i64>,
        ran: &Result<Option<String>>,
    ) -> Result<()> {
        let (outcome, job_id, error) = match ran {
            Ok(Some(job)) => (ScheduleOutcome::Started, Some(job.clone()), None),
            Ok(None) => (ScheduleOutcome::NothingChanged, None, None),
            Err(CoreError::Invalid(m)) if m == STILL_RUNNING => {
                (ScheduleOutcome::StillRunning, None, None)
            }
            Err(e) => (ScheduleOutcome::Failed, None, Some(e.to_string())),
        };
        if let Some(e) = &error {
            tracing::warn!(schedule = schedule_id, "a scheduled run failed: {e}");
        }
        self.store
            .record_schedule_run(
                schedule_id,
                ScheduleRun {
                    at,
                    next_run_at,
                    outcome: outcome.as_str().into(),
                    error,
                },
            )
            .await?;
        self.emit(Event::ScheduleRan {
            schedule_id: schedule_id.to_owned(),
            job_id,
            outcome,
        });
        Ok(())
    }

    /// A new run of `schedule`: a Transfer job, analysed, and started when Ready. `None` when
    /// nothing changed (the job is deleted again).
    async fn new_run(self: &Arc<Self>, schedule: &ScheduleRow) -> Result<Option<String>> {
        let mut options: SendOptions =
            serde_json::from_value(schedule.options.clone()).unwrap_or_default();
        options.keep_older = true;
        let profile = self.get_profile(&schedule.profile_id).await?;
        self.registry.check_profile(&profile)?;
        let id = uuid::Uuid::new_v4().to_string();
        self.store
            .insert_job(NewJob {
                id: id.clone(),
                kind: JobKind::Transfer.as_str().into(),
                profile_id: profile.id.clone(),
                state: JobState::Draft.as_str().into(),
                options: serde_json::to_value(&options)?,
                sources: schedule.sources.clone(),
                label: Some(schedule.name.clone()),
                again_of: None,
                schedule_id: Some(schedule.id.clone()),
            })
            .await?;
        let sources: Vec<String> = schedule
            .sources
            .iter()
            .map(|s| String::from_utf8_lossy(s).into_owned())
            .collect();
        self.log(
            Some(&id),
            "created",
            serde_json::json!({
                "sources": sources, "profile_id": profile.id, "options": options,
                "schedule_id": schedule.id,
            }),
        )
        .await?;
        self.analyse_and_start(&id).await
    }

    /// Analyses a run and starts it if nothing waits for the user. A run with nothing to send
    /// is deleted (`None`); one whose analysis fails ends Failed, with the reason.
    async fn analyse_and_start(self: &Arc<Self>, job_id: &str) -> Result<Option<String>> {
        let summary = match self.analyse(job_id).await {
            Ok(s) => s,
            Err(e) => {
                let mut job = self.begin(job_id, Op::Cancel).await?;
                job.go(JobState::Failed, Some(e.to_string())).await?;
                return Err(e);
            }
        };
        if summary.files == 0 {
            self.store.delete_job(job_id).await?;
            return Ok(None);
        }
        if summary.state == JobState::Ready {
            self.start(job_id).await?;
        }
        Ok(Some(job_id.to_owned()))
    }

    /// What the schedule sent before, keyed like a run's manifest: by the run's source index
    /// and the path under it.
    pub(crate) async fn sent_before(
        &self,
        schedule_id: &str,
        sources: &[Vec<u8>],
    ) -> Result<SentBefore> {
        let index: HashMap<&[u8], u32> = sources
            .iter()
            .enumerate()
            .map(|(i, s)| (s.as_slice(), i as u32))
            .collect();
        let mut sent = SentBefore::new();
        for f in self.store.sent_files(schedule_id).await? {
            if let Some(i) = index.get(f.root.as_slice()) {
                sent.insert((*i, f.raw_path), (f.size, f.mtime));
            }
        }
        Ok(sent)
    }

    /// After a run's analysis: the schedule's earlier decisions for the same names, then the
    /// rules' proposals for the names still pending. What has no proposal waits for the user.
    pub(crate) async fn settle_run_names(&self, job_id: &str, schedule_id: &str) -> Result<()> {
        let carried = self
            .store
            .carry_schedule_decisions(schedule_id, job_id)
            .await?;
        if carried > 0 {
            let kept = carried - self.recheck_carried(job_id).await?;
            self.log(
                Some(job_id),
                "decisions",
                serde_json::json!({ "carried": true, "count": kept, "schedule_id": schedule_id }),
            )
            .await?;
        }
        let names = self.job_name_plan(job_id).await?;
        let entries = self.entries(job_id).await?;
        let mut accept = Vec::new();
        for r in self.all_issues(job_id, None).await? {
            if r.issue.decision != Decision::Pending {
                continue;
            }
            let id = r.id;
            let Some(entry) = entries.get(r.issue.entry_idx as usize) else {
                continue;
            };
            let issue = decisions::decided(r);
            if issue.has_proposal() && names.check(entry, &issue, &Choice::Accept).is_ok() {
                accept.push(id);
            }
        }
        let accepted = self
            .store
            .decide_issues(job_id, accept, Decision::Accepted)
            .await?;
        if accepted > 0 {
            self.log(
                Some(job_id),
                "decisions",
                serde_json::json!({ "decision": "accepted", "count": accepted, "auto": true }),
            )
            .await?;
        }
        Ok(())
    }
}

/// Why a due time didn't start a run: the last one isn't finished.
const STILL_RUNNING: &str = "the schedule's last run isn't finished yet";

struct StillRunning;

impl From<StillRunning> for CoreError {
    fn from(_: StillRunning) -> Self {
        CoreError::Invalid(STILL_RUNNING.into())
    }
}

/// A schedule's sources as stored: absolute, sendable, none inside another.
fn schedule_sources(sources: &[String]) -> Result<Vec<Vec<u8>>> {
    if sources.is_empty() {
        return Err(CoreError::Invalid(
            "pick at least one file or folder".into(),
        ));
    }
    let paths: Vec<PathBuf> = sources.iter().map(PathBuf::from).collect();
    let mut roots = Vec::with_capacity(paths.len());
    for p in &paths {
        let abs = std::path::absolute(p)?;
        if abs.file_name().is_none() {
            return Err(CoreError::Invalid(format!(
                "{} cannot be sent",
                abs.display()
            )));
        }
        roots.push(os_bytes(abs.as_os_str()));
    }
    if let Some((inside, container)) = crate::sources::first_nested(&paths) {
        return Err(CoreError::Invalid(format!(
            "{} is already inside {}",
            inside.display(),
            container.display()
        )));
    }
    Ok(roots)
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOUR: i64 = 3_600_000;
    const MIN: i64 = 60_000;

    /// 2026-10-19 00:00 UTC, a Monday.
    const MONDAY: i64 = 1_792_368_000_000;

    fn utc() -> WallClock {
        WallClock::fixed(|| 0, 0)
    }

    #[test]
    fn next_on_chosen_days_at_a_time() {
        let r = Recurrence::Days {
            days: vec![Weekday::Mon, Weekday::Wed],
            minute: 3 * 60,
        };
        let c = utc();
        assert_eq!(c.next_run(&r, MONDAY), Some(MONDAY + 3 * HOUR));
        // At the time itself: the next one.
        assert_eq!(
            c.next_run(&r, MONDAY + 3 * HOUR),
            Some(MONDAY + 48 * HOUR + 3 * HOUR)
        );
        // After Wednesday's: next Monday.
        assert_eq!(
            c.next_run(&r, MONDAY + 48 * HOUR + 4 * HOUR),
            Some(MONDAY + 7 * 24 * HOUR + 3 * HOUR)
        );
    }

    #[test]
    fn next_every_n_hours_restarts_each_day() {
        let r = Recurrence::Hours {
            every: 5,
            minute: 30,
        };
        let c = utc();
        assert_eq!(c.next_run(&r, MONDAY), Some(MONDAY + 30 * MIN));
        assert_eq!(
            c.next_run(&r, MONDAY + 30 * MIN),
            Some(MONDAY + 5 * HOUR + 30 * MIN)
        );
        // 20:30 is the day's last; then 00:30.
        assert_eq!(
            c.next_run(&r, MONDAY + 20 * HOUR + 30 * MIN),
            Some(MONDAY + 24 * HOUR + 30 * MIN)
        );
    }

    #[test]
    fn local_time_and_stamps_follow_the_offset() {
        // UTC+1: 03:00 local is 02:00 UTC.
        let c = WallClock::fixed(|| 0, 3600);
        let r = Recurrence::Days {
            days: Weekday::ALL.to_vec(),
            minute: 3 * 60,
        };
        assert_eq!(c.next_run(&r, MONDAY), Some(MONDAY + 2 * HOUR));
        assert_eq!(c.stamp(MONDAY + 2 * HOUR), "2026-10-19 0300");
    }

    #[test]
    fn a_time_dst_skips_runs_just_after_the_gap() {
        // A time zone whose clocks go forward: a local time that doesn't exist runs at the
        // first one after it.
        use chrono::NaiveDate;
        #[derive(Clone)]
        struct Gap;
        // 2026-03-29 02:00–03:00 doesn't exist (UTC+0 → UTC+1 at 02:00 UTC).
        impl TimeZone for Gap {
            type Offset = FixedOffset;
            fn from_offset(_: &FixedOffset) -> Self {
                Gap
            }
            fn offset_from_local_date(&self, _: &NaiveDate) -> chrono::LocalResult<FixedOffset> {
                chrono::LocalResult::Single(FixedOffset::east_opt(0).unwrap())
            }
            fn offset_from_local_datetime(
                &self,
                local: &NaiveDateTime,
            ) -> chrono::LocalResult<FixedOffset> {
                let change = NaiveDate::from_ymd_opt(2026, 3, 29).unwrap();
                let (two, three) = (
                    change.and_hms_opt(2, 0, 0).unwrap(),
                    change.and_hms_opt(3, 0, 0).unwrap(),
                );
                if *local >= two && *local < three {
                    chrono::LocalResult::None
                } else if *local >= three {
                    chrono::LocalResult::Single(FixedOffset::east_opt(3600).unwrap())
                } else {
                    chrono::LocalResult::Single(FixedOffset::east_opt(0).unwrap())
                }
            }
            fn offset_from_utc_date(&self, _: &NaiveDate) -> FixedOffset {
                FixedOffset::east_opt(0).unwrap()
            }
            fn offset_from_utc_datetime(&self, utc: &NaiveDateTime) -> FixedOffset {
                let change = NaiveDate::from_ymd_opt(2026, 3, 29)
                    .unwrap()
                    .and_hms_opt(2, 0, 0)
                    .unwrap();
                FixedOffset::east_opt(if *utc >= change { 3600 } else { 0 }).unwrap()
            }
        }
        let r = Recurrence::Days {
            days: Weekday::ALL.to_vec(),
            minute: 2 * 60 + 30,
        };
        let sat = NaiveDate::from_ymd_opt(2026, 3, 28)
            .unwrap()
            .and_hms_opt(12, 0, 0)
            .unwrap()
            .and_utc()
            .timestamp_millis();
        // 02:30 on the 29th doesn't exist: 03:00 local (02:00 UTC) instead.
        let sunday_3am = NaiveDate::from_ymd_opt(2026, 3, 29)
            .unwrap()
            .and_hms_opt(2, 0, 0)
            .unwrap()
            .and_utc()
            .timestamp_millis();
        assert_eq!(next_in(&Gap, &r, sat), Some(sunday_3am));
    }
}
