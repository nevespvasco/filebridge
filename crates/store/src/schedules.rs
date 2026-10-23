//! Scheduled sends (docs/adr/0019): the schedules, the files each one has sent, and the name
//! decisions its runs made. `core` decides when a schedule runs and what changed; this only
//! keeps the rows.

use rusqlite::{Connection, OptionalExtension, Row, params};

use crate::{Result, Store, StoreError, now_ms};

/// A schedule. `options` and `recurrence` are `core`'s types as JSON.
#[derive(Debug, Clone, PartialEq)]
pub struct ScheduleRow {
    pub id: String,
    pub name: String,
    pub profile_id: String,
    pub options: serde_json::Value,
    pub recurrence: serde_json::Value,
    pub enabled: bool,
    /// Unix ms; `None` while disabled.
    pub next_run_at: Option<i64>,
    pub last_run_at: Option<i64>,
    pub last_outcome: Option<String>,
    pub last_error: Option<String>,
    /// A watched schedule's last change seen (docs/adr/0023).
    pub last_change_at: Option<i64>,
    /// A watched schedule's first change not in a run yet.
    pub pending_since: Option<i64>,
    pub created_at: i64,
    pub updated_at: i64,
    /// Source roots as raw path bytes, in the order the user picked them.
    pub sources: Vec<Vec<u8>>,
}

/// A file a schedule has sent (or skipped by a name decision), as it was then.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SentFile {
    /// The source root it was under (as in `job_sources`).
    pub root: Vec<u8>,
    /// Relative to the root's parent (as in the manifest).
    pub raw_path: Vec<u8>,
    pub size: u64,
    pub mtime: Option<i64>,
    /// Empty for a file skipped by a name decision.
    pub dest_path: String,
}

/// What a due time did: saved on the schedule with its next time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScheduleRun {
    pub at: i64,
    pub next_run_at: Option<i64>,
    pub outcome: String,
    pub error: Option<String>,
    /// A run took the changes seen so far: `pending_since` is cleared.
    pub took_changes: bool,
}

const COLUMNS: &str = "id, name, profile_id, options, recurrence, enabled, next_run_at, \
                       last_run_at, last_outcome, last_error, created_at, updated_at, \
                       last_change_at, pending_since";

type Raw = (ScheduleRow, String, String);

fn from_row(row: &Row<'_>) -> rusqlite::Result<Raw> {
    Ok((
        ScheduleRow {
            id: row.get(0)?,
            name: row.get(1)?,
            profile_id: row.get(2)?,
            options: serde_json::Value::Null,
            recurrence: serde_json::Value::Null,
            enabled: row.get(5)?,
            next_run_at: row.get(6)?,
            last_run_at: row.get(7)?,
            last_outcome: row.get(8)?,
            last_error: row.get(9)?,
            created_at: row.get(10)?,
            updated_at: row.get(11)?,
            last_change_at: row.get(12)?,
            pending_since: row.get(13)?,
            sources: Vec::new(),
        },
        row.get(3)?,
        row.get(4)?,
    ))
}

fn load(c: &Connection, raw: Raw) -> Result<ScheduleRow> {
    let (mut s, options, recurrence) = raw;
    s.options = serde_json::from_str(&options)?;
    s.recurrence = serde_json::from_str(&recurrence)?;
    let mut stmt = c.prepare_cached(
        "SELECT path FROM schedule_sources WHERE schedule_id = ?1 ORDER BY position",
    )?;
    s.sources = stmt
        .query_map([&s.id], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    Ok(s)
}

fn query(c: &Connection, sql: &str, params: impl rusqlite::Params) -> Result<Vec<ScheduleRow>> {
    let mut stmt = c.prepare(sql)?;
    let raws = stmt
        .query_map(params, from_row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    raws.into_iter().map(|r| load(c, r)).collect()
}

fn not_found(id: &str) -> StoreError {
    StoreError::NotFound(format!("schedule {id}"))
}

impl Store {
    /// Saves a new schedule with its sources. `created_at`/`updated_at` are set here.
    pub async fn insert_schedule(&self, s: ScheduleRow) -> Result<()> {
        let options = serde_json::to_string(&s.options)?;
        let recurrence = serde_json::to_string(&s.recurrence)?;
        self.call(move |c| {
            let now = now_ms();
            let tx = c.transaction()?;
            tx.execute(
                "INSERT INTO schedules (id, name, profile_id, options, recurrence, enabled,
                                        next_run_at, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8)",
                params![
                    s.id,
                    s.name,
                    s.profile_id,
                    options,
                    recurrence,
                    s.enabled,
                    s.next_run_at,
                    now
                ],
            )?;
            {
                let mut stmt = tx.prepare(
                    "INSERT INTO schedule_sources (schedule_id, position, path)
                     VALUES (?1, ?2, ?3)",
                )?;
                for (i, path) in s.sources.iter().enumerate() {
                    stmt.execute(params![s.id, i as i64, path])?;
                }
            }
            tx.commit()?;
            Ok(())
        })
        .await
    }

    /// Replaces what may change once a schedule is saved: its name, options, recurrence,
    /// `enabled` and next time. Its sources and profile stay.
    pub async fn update_schedule(&self, s: ScheduleRow) -> Result<()> {
        let options = serde_json::to_string(&s.options)?;
        let recurrence = serde_json::to_string(&s.recurrence)?;
        self.call(move |c| {
            let n = c.execute(
                "UPDATE schedules SET name = ?2, options = ?3, recurrence = ?4, enabled = ?5,
                     next_run_at = ?6, updated_at = ?7
                 WHERE id = ?1",
                params![
                    s.id,
                    s.name,
                    options,
                    recurrence,
                    s.enabled,
                    s.next_run_at,
                    now_ms()
                ],
            )?;
            if n == 0 {
                return Err(not_found(&s.id));
            }
            Ok(())
        })
        .await
    }

    pub async fn get_schedule(&self, id: &str) -> Result<ScheduleRow> {
        let id = id.to_owned();
        self.call(move |c| {
            let raw = c
                .query_row(
                    &format!("SELECT {COLUMNS} FROM schedules WHERE id = ?1"),
                    [&id],
                    from_row,
                )
                .optional()?
                .ok_or_else(|| not_found(&id))?;
            load(c, raw)
        })
        .await
    }

    /// By name, then id.
    pub async fn list_schedules(&self) -> Result<Vec<ScheduleRow>> {
        self.call(|c| {
            query(
                c,
                &format!("SELECT {COLUMNS} FROM schedules ORDER BY name COLLATE NOCASE, id"),
                [],
            )
        })
        .await
    }

    /// Enabled schedules whose next time is at or before `now`, earliest first.
    pub async fn due_schedules(&self, now: i64) -> Result<Vec<ScheduleRow>> {
        self.call(move |c| {
            query(
                c,
                &format!(
                    "SELECT {COLUMNS} FROM schedules
                     WHERE enabled = 1 AND next_run_at IS NOT NULL AND next_run_at <= ?1
                     ORDER BY next_run_at, id"
                ),
                [now],
            )
        })
        .await
    }

    /// The earliest next time of an enabled schedule.
    pub async fn next_schedule_time(&self) -> Result<Option<i64>> {
        self.call(|c| {
            Ok(c.query_row(
                "SELECT MIN(next_run_at) FROM schedules WHERE enabled = 1",
                [],
                |r| r.get(0),
            )?)
        })
        .await
    }

    /// The ids of the schedules that send to `profile_id`.
    pub async fn schedules_of_profile(&self, profile_id: &str) -> Result<Vec<String>> {
        let profile_id = profile_id.to_owned();
        self.call(move |c| {
            let mut stmt =
                c.prepare("SELECT id FROM schedules WHERE profile_id = ?1 ORDER BY id")?;
            let ids = stmt
                .query_map([&profile_id], |r| r.get(0))?
                .collect::<rusqlite::Result<_>>()?;
            Ok(ids)
        })
        .await
    }

    /// Records what a due time did and when the schedule runs next.
    pub async fn record_schedule_run(&self, id: &str, run: ScheduleRun) -> Result<()> {
        let id = id.to_owned();
        self.call(move |c| {
            let n = c.execute(
                "UPDATE schedules SET last_run_at = ?2, next_run_at = ?3, last_outcome = ?4,
                     last_error = ?5,
                     pending_since = CASE WHEN ?6 THEN NULL ELSE pending_since END
                 WHERE id = ?1",
                params![
                    id,
                    run.at,
                    run.next_run_at,
                    run.outcome,
                    run.error,
                    run.took_changes
                ],
            )?;
            if n == 0 {
                return Err(not_found(&id));
            }
            Ok(())
        })
        .await
    }

    /// Notes a change seen in a watched schedule's sources at `at` (docs/adr/0023): its last
    /// change, its first change not in a run yet (kept if there is one), and its next time.
    pub async fn note_schedule_change(
        &self,
        id: &str,
        at: i64,
        next_run_at: Option<i64>,
    ) -> Result<()> {
        let id = id.to_owned();
        self.call(move |c| {
            let n = c.execute(
                "UPDATE schedules SET last_change_at = ?2,
                     pending_since = COALESCE(pending_since, ?2), next_run_at = ?3
                 WHERE id = ?1",
                params![id, at, next_run_at],
            )?;
            if n == 0 {
                return Err(not_found(&id));
            }
            Ok(())
        })
        .await
    }

    /// Moves a schedule's next time without recording a run (a watched schedule waiting for
    /// its last run, or for the send window).
    pub async fn set_next_run(&self, id: &str, next_run_at: Option<i64>) -> Result<()> {
        let id = id.to_owned();
        self.call(move |c| {
            let n = c.execute(
                "UPDATE schedules SET next_run_at = ?2 WHERE id = ?1",
                params![id, next_run_at],
            )?;
            if n == 0 {
                return Err(not_found(&id));
            }
            Ok(())
        })
        .await
    }

    /// Forgets a schedule, the files it sent and its decisions. Its runs stay, as ordinary jobs.
    pub async fn delete_schedule(&self, id: &str) -> Result<()> {
        let id = id.to_owned();
        self.call(move |c| {
            let n = c.execute("DELETE FROM schedules WHERE id = ?1", [&id])?;
            if n == 0 {
                return Err(not_found(&id));
            }
            Ok(())
        })
        .await
    }

    /// Every file the schedule has sent, as it was when sent.
    pub async fn sent_files(&self, schedule_id: &str) -> Result<Vec<SentFile>> {
        let schedule_id = schedule_id.to_owned();
        self.call(move |c| {
            let mut stmt = c.prepare(
                "SELECT root, raw_path, size, mtime, dest_path FROM schedule_files
                 WHERE schedule_id = ?1",
            )?;
            let rows = stmt
                .query_map([&schedule_id], |r| {
                    Ok(SentFile {
                        root: r.get(0)?,
                        raw_path: r.get(1)?,
                        size: r.get(2)?,
                        mtime: r.get(3)?,
                        dest_path: r.get(4)?,
                    })
                })?
                .collect::<rusqlite::Result<_>>()?;
            Ok(rows)
        })
        .await
    }

    /// How many files the schedule has sent (skipped ones left out).
    pub async fn sent_file_count(&self, schedule_id: &str) -> Result<u64> {
        let schedule_id = schedule_id.to_owned();
        self.call(move |c| {
            Ok(c.query_row(
                "SELECT COUNT(*) FROM schedule_files WHERE schedule_id = ?1 AND dest_path <> ''",
                [&schedule_id],
                |r| r.get(0),
            )?)
        })
        .await
    }

    /// Keeps the decided name issues of a run (one of the schedule's jobs) for its next runs,
    /// per source path and rule. Pending ones are left out.
    pub async fn save_schedule_decisions(&self, schedule_id: &str, job_id: &str) -> Result<u64> {
        let (schedule_id, job_id) = (schedule_id.to_owned(), job_id.to_owned());
        self.call(move |c| {
            let n = c.execute(
                "INSERT INTO schedule_decisions
                     (schedule_id, root, raw_path, rule, decision, custom_name)
                 SELECT ?1, s.path, m.raw_path, i.rule, i.decision, i.custom_name
                 FROM issues i
                 JOIN manifest_entries m ON m.job_id = i.job_id AND m.idx = i.entry_idx
                 JOIN job_sources s ON s.job_id = m.job_id AND s.position = m.source
                 WHERE i.job_id = ?2 AND i.decision <> 'pending'
                 ON CONFLICT (schedule_id, root, raw_path, rule) DO UPDATE SET
                     decision = excluded.decision, custom_name = excluded.custom_name",
                params![schedule_id, job_id],
            )?;
            Ok(n as u64)
        })
        .await
    }

    /// Gives a run's pending issues the decisions its schedule kept for the same source path
    /// and rule, marked as carried over. Returns how many changed.
    pub async fn carry_schedule_decisions(&self, schedule_id: &str, job_id: &str) -> Result<u64> {
        let (schedule_id, job_id) = (schedule_id.to_owned(), job_id.to_owned());
        self.call(move |c| {
            let n = c.execute(
                "UPDATE issues SET decision = d.decision, custom_name = d.custom_name,
                     decided_at = ?3, carried_over = 1
                 FROM schedule_decisions d
                 JOIN manifest_entries m ON m.job_id = ?2
                 JOIN job_sources s ON s.job_id = m.job_id AND s.position = m.source
                 WHERE issues.job_id = ?2 AND issues.decision = 'pending'
                   AND m.idx = issues.entry_idx
                   AND d.schedule_id = ?1 AND d.root = s.path AND d.raw_path = m.raw_path
                   AND d.rule = issues.rule",
                params![schedule_id, job_id, now_ms()],
            )?;
            Ok(n as u64)
        })
        .await
    }

    /// The schedule's newest run, if any: its id, state, and whether it is finished.
    pub async fn latest_run(&self, schedule_id: &str) -> Result<Option<(String, String, bool)>> {
        let schedule_id = schedule_id.to_owned();
        self.call(move |c| {
            Ok(c.query_row(
                "SELECT id, state, finished_at IS NOT NULL FROM jobs WHERE schedule_id = ?1
                 ORDER BY created_at DESC, id DESC LIMIT 1",
                [&schedule_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?)
        })
        .await
    }

    /// The schedule's runs not in a terminal state, oldest first.
    pub async fn unfinished_runs(&self, schedule_id: &str) -> Result<Vec<(String, String)>> {
        let schedule_id = schedule_id.to_owned();
        self.call(move |c| {
            let mut stmt = c.prepare(
                "SELECT id, state FROM jobs WHERE schedule_id = ?1 AND finished_at IS NULL
                 ORDER BY created_at, id",
            )?;
            let rows = stmt
                .query_map([&schedule_id], |r| Ok((r.get(0)?, r.get(1)?)))?
                .collect::<rusqlite::Result<_>>()?;
            Ok(rows)
        })
        .await
    }

    /// Deletes a job with everything it holds (a run that found nothing to send).
    pub async fn delete_job(&self, id: &str) -> Result<()> {
        let id = id.to_owned();
        self.call(move |c| {
            let n = c.execute("DELETE FROM jobs WHERE id = ?1", [&id])?;
            if n == 0 {
                return Err(StoreError::NotFound(format!("job {id}")));
            }
            Ok(())
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::store_with_job;
    use crate::{Decision, EntryState, NewEntry, NewIssue, NewJob};

    fn schedule(id: &str) -> ScheduleRow {
        ScheduleRow {
            id: id.into(),
            name: format!("Nightly {id}"),
            profile_id: "p1".into(),
            options: serde_json::json!({ "verify": true }),
            recurrence: serde_json::json!({ "kind": "hours", "every": 6, "minute": 0 }),
            enabled: true,
            next_run_at: Some(1_000),
            last_run_at: None,
            last_outcome: None,
            last_error: None,
            last_change_at: None,
            pending_since: None,
            created_at: 0,
            updated_at: 0,
            sources: vec![b"/home/u/Fotos".to_vec(), b"/home/u/Patol\xf3gica".to_vec()],
        }
    }

    async fn run_of(store: &Store, id: &str, schedule_id: &str) {
        store
            .insert_job(NewJob {
                id: id.into(),
                kind: "transfer".into(),
                profile_id: "p1".into(),
                state: "draft".into(),
                options: serde_json::json!({}),
                sources: vec![b"/home/u/Fotos".to_vec()],
                label: None,
                again_of: None,
                schedule_id: Some(schedule_id.into()),
            })
            .await
            .unwrap();
        let entry = |idx: u64, path: &[u8], is_dir: bool| NewEntry {
            idx,
            source: 0,
            raw_path: path.to_vec(),
            parent_idx: idx.checked_sub(1),
            is_dir,
            size: if is_dir { 0 } else { 3 },
            mtime: Some(7),
            hash: None,
            hash_algo: None,
            category: "other".into(),
        };
        store
            .insert_entries(
                id,
                vec![entry(0, b"Fotos", true), entry(1, b"Fotos/a:b.jpg", false)],
            )
            .await
            .unwrap();
        store
            .set_dest_paths(
                id,
                vec![(0, Some("Fotos".into())), (1, Some("Fotos/a_b.jpg".into()))],
            )
            .await
            .unwrap();
        store
            .insert_issues(
                id,
                vec![NewIssue {
                    entry_idx: 1,
                    rule: "forbidden-chars".into(),
                    severity: "needs_decision".into(),
                    message_key: "k".into(),
                    before: b"a:b.jpg".to_vec(),
                    after: b"a_b.jpg".to_vec(),
                    decision: Decision::Pending,
                }],
            )
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn schedules_round_trip_and_fall_due() {
        let (store, _) = store_with_job().await;
        store.insert_schedule(schedule("s1")).await.unwrap();
        let mut later = schedule("s2");
        later.next_run_at = Some(5_000);
        store.insert_schedule(later).await.unwrap();

        let got = store.get_schedule("s1").await.unwrap();
        assert_eq!(got.sources[1], b"/home/u/Patol\xf3gica");
        assert_eq!(got.options["verify"], true);
        assert_eq!(got.recurrence["every"], 6);
        assert!(got.created_at > 0);
        assert_eq!(store.list_schedules().await.unwrap().len(), 2);

        let due: Vec<_> = store.due_schedules(1_000).await.unwrap();
        assert_eq!(
            due.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(),
            ["s1"]
        );
        assert_eq!(store.next_schedule_time().await.unwrap(), Some(1_000));

        let mut off = got.clone();
        off.enabled = false;
        off.next_run_at = None;
        off.name = "Renamed".into();
        store.update_schedule(off).await.unwrap();
        assert!(store.due_schedules(10_000).await.unwrap()[0].id == "s2");
        assert_eq!(store.get_schedule("s1").await.unwrap().name, "Renamed");

        store
            .record_schedule_run(
                "s2",
                ScheduleRun {
                    at: 5_100,
                    next_run_at: Some(9_000),
                    outcome: "still_running".into(),
                    error: None,
                    took_changes: false,
                },
            )
            .await
            .unwrap();
        let s2 = store.get_schedule("s2").await.unwrap();
        assert_eq!((s2.last_run_at, s2.next_run_at), (Some(5_100), Some(9_000)));
        assert_eq!(s2.last_outcome.as_deref(), Some("still_running"));
        assert_eq!(store.schedules_of_profile("p1").await.unwrap().len(), 2);

        assert!(matches!(
            store.get_schedule("nope").await,
            Err(StoreError::NotFound(_))
        ));
    }

    #[tokio::test]
    async fn changes_keep_the_first_until_a_run_takes_them() {
        let (store, _) = store_with_job().await;
        store.insert_schedule(schedule("s1")).await.unwrap();
        store
            .note_schedule_change("s1", 2_000, Some(2_060))
            .await
            .unwrap();
        store
            .note_schedule_change("s1", 2_030, Some(2_090))
            .await
            .unwrap();
        let s = store.get_schedule("s1").await.unwrap();
        assert_eq!(
            (s.last_change_at, s.pending_since, s.next_run_at),
            (Some(2_030), Some(2_000), Some(2_090))
        );
        store.set_next_run("s1", Some(5_000)).await.unwrap();
        assert_eq!(store.next_schedule_time().await.unwrap(), Some(5_000));

        let run = |took_changes| ScheduleRun {
            at: 2_100,
            next_run_at: Some(9_000),
            outcome: "started".into(),
            error: None,
            took_changes,
        };
        store.record_schedule_run("s1", run(false)).await.unwrap();
        assert_eq!(
            store.get_schedule("s1").await.unwrap().pending_since,
            Some(2_000)
        );
        store.record_schedule_run("s1", run(true)).await.unwrap();
        let s = store.get_schedule("s1").await.unwrap();
        assert_eq!((s.last_change_at, s.pending_since), (Some(2_030), None));
        assert!(matches!(
            store.set_next_run("nope", None).await,
            Err(StoreError::NotFound(_))
        ));
    }

    #[tokio::test]
    async fn sent_files_are_recorded_with_the_done_state() {
        let (store, _) = store_with_job().await;
        store.insert_schedule(schedule("s1")).await.unwrap();
        run_of(&store, "r1", "s1").await;
        assert!(store.sent_files("s1").await.unwrap().is_empty());

        // Failed: not recorded. The folder never is. Skipped: recorded, with no destination.
        store
            .set_transfer_states("r1", vec![(1, EntryState::Failed, Some("x".into()))])
            .await
            .unwrap();
        store
            .set_transfer_states("r1", vec![(0, EntryState::Done, None)])
            .await
            .unwrap();
        assert!(store.sent_files("s1").await.unwrap().is_empty());

        store
            .set_transfer_states("r1", vec![(1, EntryState::Skipped, None)])
            .await
            .unwrap();
        assert_eq!(store.sent_files("s1").await.unwrap()[0].dest_path, "");
        assert_eq!(store.sent_file_count("s1").await.unwrap(), 0);
        store
            .set_transfer_states("r1", vec![(1, EntryState::Done, None)])
            .await
            .unwrap();
        let sent = store.sent_files("s1").await.unwrap();
        assert_eq!(
            sent,
            [SentFile {
                root: b"/home/u/Fotos".to_vec(),
                raw_path: b"Fotos/a:b.jpg".to_vec(),
                size: 3,
                mtime: Some(7),
                dest_path: "Fotos/a_b.jpg".into(),
            }]
        );
        assert_eq!(store.sent_file_count("s1").await.unwrap(), 1);

        // An ordinary job's files are not any schedule's.
        store
            .set_transfer_states("j1", vec![(0, EntryState::Done, None)])
            .await
            .unwrap();
        assert_eq!(store.sent_file_count("s1").await.unwrap(), 1);
    }

    #[tokio::test]
    async fn decisions_are_kept_and_carried_by_path_and_rule() {
        let (store, _) = store_with_job().await;
        store.insert_schedule(schedule("s1")).await.unwrap();
        run_of(&store, "r1", "s1").await;
        assert_eq!(store.save_schedule_decisions("s1", "r1").await.unwrap(), 0);
        let issue = store.issues_page("r1", None, None, 10).await.unwrap()[0].id;
        store
            .decide_issue("r1", issue, Decision::Edited, Some("ab.jpg".into()))
            .await
            .unwrap();
        assert_eq!(store.save_schedule_decisions("s1", "r1").await.unwrap(), 1);

        run_of(&store, "r2", "s1").await;
        assert_eq!(store.carry_schedule_decisions("s1", "r2").await.unwrap(), 1);
        let carried = &store.issues_page("r2", None, None, 10).await.unwrap()[0];
        assert_eq!(carried.issue.decision, Decision::Edited);
        assert_eq!(carried.custom_name.as_deref(), Some("ab.jpg"));
        assert!(carried.carried_over);
        // Already decided: not carried again.
        assert_eq!(store.carry_schedule_decisions("s1", "r2").await.unwrap(), 0);
    }

    #[tokio::test]
    async fn runs_and_removal() {
        let (store, _) = store_with_job().await;
        store.insert_schedule(schedule("s1")).await.unwrap();
        assert_eq!(store.latest_run("s1").await.unwrap(), None);
        run_of(&store, "r1", "s1").await;
        assert_eq!(
            store.latest_run("s1").await.unwrap(),
            Some(("r1".into(), "draft".into(), false))
        );
        assert_eq!(store.unfinished_runs("s1").await.unwrap().len(), 1);
        store
            .set_transfer_states("r1", vec![(1, EntryState::Done, None)])
            .await
            .unwrap();

        // The runs stay as ordinary jobs; the schedule's rows go.
        store.delete_schedule("s1").await.unwrap();
        assert_eq!(store.get_job("r1").await.unwrap().schedule_id, None);
        assert!(store.sent_files("s1").await.unwrap().is_empty());
        assert!(matches!(
            store.delete_schedule("s1").await,
            Err(StoreError::NotFound(_))
        ));

        store.delete_job("r1").await.unwrap();
        assert!(matches!(
            store.get_job("r1").await,
            Err(StoreError::NotFound(_))
        ));
    }
}
