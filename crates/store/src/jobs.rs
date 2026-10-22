use rusqlite::types::Value;
use rusqlite::{Connection, OptionalExtension, Row, params};

use crate::{Result, Store, StoreError, now_ms};

/// Which jobs [`Store::list_jobs_filtered`] returns: all of these must hold.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct JobQuery {
    /// Any of these states (as text); empty = any.
    pub states: Vec<String>,
    pub profile_id: Option<String>,
    /// A substring of the label, a source path or the profile name.
    pub text: Option<String>,
    /// Only these jobs (e.g. the reclaimable ones `core` worked out).
    pub ids: Option<Vec<String>>,
    pub order: JobOrder,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum JobOrder {
    #[default]
    Newest,
    Oldest,
    /// Most bytes first.
    Largest,
}

/// A new job. `state` and `kind` are `core`'s enums as text.
#[derive(Debug, Clone)]
pub struct NewJob {
    pub id: String,
    pub kind: String,
    pub profile_id: String,
    pub state: String,
    pub options: serde_json::Value,
    /// Source roots as raw path bytes, in the order the user picked them.
    pub sources: Vec<Vec<u8>>,
    /// The user's name for the job.
    pub label: Option<String>,
    /// The job this one repeats (Send again).
    pub again_of: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct JobRow {
    pub id: String,
    pub kind: String,
    pub profile_id: String,
    pub state: String,
    pub options: serde_json::Value,
    pub error: Option<String>,
    /// Consecutive failed attempts, for backoff.
    pub attempt: u32,
    pub next_retry_at: Option<i64>,
    pub created_at: i64,
    pub updated_at: i64,
    /// Set once the job reaches a terminal state; unfinished jobs resume on startup.
    pub finished_at: Option<i64>,
    pub sources: Vec<Vec<u8>>,
    /// Warnings of the stages finished so far.
    pub warnings: Vec<String>,
    /// Verify rounds that found differences and sent files again.
    pub verify_rounds: u32,
    pub label: Option<String>,
    pub again_of: Option<String>,
    /// When the user hid the job from the reclaimable total (docs/design/specs/D10).
    pub reclaim_dismissed_at: Option<i64>,
    /// What the last source re-check found (JSON, D10).
    pub reclaim_check: Option<serde_json::Value>,
}

/// A job's move to a new state, written in one transaction with what the stage that just
/// ended produced, and logged as a `state` event.
#[derive(Debug, Clone, Default)]
pub struct StateChange {
    /// The state the caller saw. The change is refused if the job is no longer in it.
    pub from: String,
    pub to: String,
    /// Replaces the previous error (`None` clears it).
    pub error: Option<String>,
    /// A terminal state: the job is finished and never resumed.
    pub finished: bool,
    /// Warnings of the stage that just ended, added to the job's.
    pub warnings: Vec<String>,
    /// The stage that just ended was a verify round that sends files again.
    pub verify_round: bool,
}

const COLUMNS: &str = "id, kind, profile_id, state, options, error, attempt, next_retry_at, \
                       created_at, updated_at, finished_at, warnings, verify_rounds, label, \
                       again_of, reclaim_dismissed_at, reclaim_check";

/// A row, plus its options and warnings as JSON text.
type RawJob = (JobRow, String, String);

fn from_row(row: &Row<'_>) -> rusqlite::Result<RawJob> {
    Ok((
        JobRow {
            id: row.get(0)?,
            kind: row.get(1)?,
            profile_id: row.get(2)?,
            state: row.get(3)?,
            options: serde_json::Value::Null,
            error: row.get(5)?,
            attempt: row.get(6)?,
            next_retry_at: row.get(7)?,
            created_at: row.get(8)?,
            updated_at: row.get(9)?,
            finished_at: row.get(10)?,
            sources: Vec::new(),
            warnings: Vec::new(),
            verify_rounds: row.get(12)?,
            label: row.get(13)?,
            again_of: row.get(14)?,
            reclaim_dismissed_at: row.get(15)?,
            reclaim_check: row
                .get::<_, Option<String>>(16)?
                .and_then(|s| serde_json::from_str(&s).ok()),
        },
        row.get(4)?,
        row.get(11)?,
    ))
}

fn load(c: &Connection, raw: RawJob) -> Result<JobRow> {
    let (mut job, options, warnings) = raw;
    job.options = serde_json::from_str(&options)?;
    job.warnings = serde_json::from_str(&warnings)?;
    let mut stmt =
        c.prepare_cached("SELECT path FROM job_sources WHERE job_id = ?1 ORDER BY position")?;
    job.sources = stmt
        .query_map([&job.id], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    Ok(job)
}

fn query_jobs(c: &Connection, sql: &str, params: impl rusqlite::Params) -> Result<Vec<JobRow>> {
    let mut stmt = c.prepare(sql)?;
    let raws = stmt
        .query_map(params, from_row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    raws.into_iter().map(|r| load(c, r)).collect()
}

fn expect_one(n: usize, id: &str) -> Result<()> {
    if n == 0 {
        Err(StoreError::NotFound(format!("job {id}")))
    } else {
        Ok(())
    }
}

impl Store {
    pub async fn insert_job(&self, job: NewJob) -> Result<()> {
        let options = serde_json::to_string(&job.options)?;
        self.call(move |c| {
            let now = now_ms();
            let tx = c.transaction()?;
            tx.execute(
                "INSERT INTO jobs (id, kind, profile_id, state, options, created_at, updated_at,
                                   label, again_of)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6, ?7, ?8)",
                params![
                    job.id,
                    job.kind,
                    job.profile_id,
                    job.state,
                    options,
                    now,
                    job.label,
                    job.again_of
                ],
            )?;
            {
                let mut stmt = tx.prepare(
                    "INSERT INTO job_sources (job_id, position, path) VALUES (?1, ?2, ?3)",
                )?;
                for (i, path) in job.sources.iter().enumerate() {
                    stmt.execute(params![job.id, i as i64, path])?;
                }
            }
            tx.commit()?;
            Ok(())
        })
        .await
    }

    pub async fn get_job(&self, id: &str) -> Result<JobRow> {
        let id = id.to_owned();
        self.call(move |c| {
            let raw = c
                .query_row(
                    &format!("SELECT {COLUMNS} FROM jobs WHERE id = ?1"),
                    [&id],
                    from_row,
                )
                .optional()?
                .ok_or_else(|| StoreError::NotFound(format!("job {id}")))?;
            load(c, raw)
        })
        .await
    }

    /// Newest first.
    pub async fn list_jobs(&self, limit: u32, offset: u32) -> Result<Vec<JobRow>> {
        self.call(move |c| {
            query_jobs(
                c,
                &format!(
                    "SELECT {COLUMNS} FROM jobs ORDER BY created_at DESC, id LIMIT ?1 OFFSET ?2"
                ),
                params![limit, offset],
            )
        })
        .await
    }

    /// Jobs not in a terminal state, oldest first: what `core` resumes on startup.
    pub async fn list_unfinished_jobs(&self) -> Result<Vec<JobRow>> {
        self.call(|c| {
            query_jobs(
                c,
                &format!(
                    "SELECT {COLUMNS} FROM jobs WHERE finished_at IS NULL ORDER BY created_at, id"
                ),
                [],
            )
        })
        .await
    }

    /// The jobs to `profile_id` not in a terminal state (drafts included), oldest first.
    pub async fn unfinished_jobs_of_profile(&self, profile_id: &str) -> Result<Vec<JobRow>> {
        let profile_id = profile_id.to_owned();
        self.call(move |c| {
            query_jobs(
                c,
                &format!(
                    "SELECT {COLUMNS} FROM jobs WHERE finished_at IS NULL AND profile_id = ?1
                     ORDER BY created_at, id"
                ),
                [&profile_id],
            )
        })
        .await
    }

    /// Records a state change, the stage outcome and its `state` event, in one transaction.
    /// `Invalid` if the job is no longer in `change.from` (someone else moved it).
    pub async fn change_job_state(&self, id: &str, change: StateChange) -> Result<()> {
        let id = id.to_owned();
        let event = serde_json::to_string(&serde_json::json!({
            "from": change.from, "to": change.to, "error": change.error,
        }))?;
        self.call(move |c| {
            let now = now_ms();
            let tx = c.transaction()?;
            let current: Option<(String, String)> = tx
                .query_row(
                    "SELECT state, warnings FROM jobs WHERE id = ?1",
                    [&id],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()?;
            let Some((state, warnings)) = current else {
                return Err(StoreError::NotFound(format!("job {id}")));
            };
            if state != change.from {
                return Err(StoreError::Invalid(format!(
                    "job {id} is {state}, not {}",
                    change.from
                )));
            }
            let mut all: Vec<String> = serde_json::from_str(&warnings)?;
            all.extend(change.warnings);
            tx.execute(
                "UPDATE jobs SET state = ?2, error = ?3, updated_at = ?4,
                     finished_at = CASE WHEN ?5 THEN ?4 ELSE finished_at END,
                     next_retry_at = CASE WHEN ?5 THEN NULL ELSE next_retry_at END,
                     warnings = ?6, verify_rounds = verify_rounds + ?7
                 WHERE id = ?1",
                params![
                    id,
                    change.to,
                    change.error,
                    now,
                    change.finished,
                    serde_json::to_string(&all)?,
                    u32::from(change.verify_round)
                ],
            )?;
            tx.execute(
                "INSERT INTO events (job_id, at, kind, data) VALUES (?1, ?2, 'state', ?3)",
                params![id, now, event],
            )?;
            tx.commit()?;
            Ok(())
        })
        .await
    }

    /// Records the backoff position: `attempt` 0 and `None` once the job makes progress again.
    pub async fn set_job_retry(
        &self,
        id: &str,
        attempt: u32,
        next_retry_at: Option<i64>,
    ) -> Result<()> {
        let id = id.to_owned();
        self.call(move |c| {
            let n = c.execute(
                "UPDATE jobs SET attempt = ?2, next_retry_at = ?3, updated_at = ?4 WHERE id = ?1",
                params![id, attempt, next_retry_at, now_ms()],
            )?;
            expect_one(n, &id)
        })
        .await
    }

    /// Keeps what a re-check of the job's sources found (D10).
    pub async fn set_reclaim_check(&self, id: &str, check: serde_json::Value) -> Result<()> {
        let (id, check) = (id.to_owned(), serde_json::to_string(&check)?);
        self.call(move |c| {
            let n = c.execute(
                "UPDATE jobs SET reclaim_check = ?2 WHERE id = ?1",
                params![id, check],
            )?;
            expect_one(n, &id)
        })
        .await
    }

    /// Hides the job from the reclaimable total (`Some(time)`), or shows it again (`None`).
    pub async fn set_reclaim_dismissed(&self, id: &str, at: Option<i64>) -> Result<()> {
        let id = id.to_owned();
        self.call(move |c| {
            let n = c.execute(
                "UPDATE jobs SET reclaim_dismissed_at = ?2 WHERE id = ?1",
                params![id, at],
            )?;
            expect_one(n, &id)
        })
        .await
    }

    /// Ids of completed jobs, newest first: the candidates for a reclaimable source.
    pub async fn completed_job_ids(&self) -> Result<Vec<String>> {
        self.call(|c| {
            let mut stmt = c.prepare_cached(
                "SELECT id FROM jobs WHERE state = 'completed' ORDER BY created_at DESC",
            )?;
            let ids = stmt.query_map([], |r| r.get(0))?;
            Ok(ids.collect::<rusqlite::Result<_>>()?)
        })
        .await
    }

    /// Names a job; `None` clears the name.
    pub async fn set_job_label(&self, id: &str, label: Option<String>) -> Result<()> {
        let id = id.to_owned();
        self.call(move |c| {
            let n = c.execute(
                "UPDATE jobs SET label = ?2, updated_at = ?3 WHERE id = ?1",
                params![id, label, now_ms()],
            )?;
            expect_one(n, &id)
        })
        .await
    }

    /// A page of jobs matching `q`, and how many match in all (the Transfers list, D06).
    pub async fn list_jobs_filtered(
        &self,
        q: JobQuery,
        limit: u32,
        offset: u32,
    ) -> Result<(Vec<JobRow>, u64)> {
        self.call(move |c| {
            let mut clauses: Vec<String> = Vec::new();
            let mut args: Vec<Value> = Vec::new();
            if !q.states.is_empty() {
                clauses.push(format!(
                    "state IN ({})",
                    vec!["?"; q.states.len()].join(", ")
                ));
                args.extend(q.states.iter().cloned().map(Value::Text));
            }
            if let Some(p) = &q.profile_id {
                clauses.push("profile_id = ?".into());
                args.push(Value::Text(p.clone()));
            }
            if let Some(text) = q.text.as_deref().filter(|t| !t.is_empty()) {
                let pattern = format!("%{}%", crate::manifest::like_escape(text));
                clauses.push(
                    "(label LIKE ? ESCAPE '\\' \
                      OR EXISTS (SELECT 1 FROM job_sources s WHERE s.job_id = jobs.id \
                                 AND CAST(s.path AS TEXT) LIKE ? ESCAPE '\\') \
                      OR profile_id IN (SELECT id FROM profiles WHERE name LIKE ? ESCAPE '\\'))"
                        .into(),
                );
                args.extend(std::iter::repeat_n(Value::Text(pattern), 3));
            }
            if let Some(only) = &q.ids {
                if only.is_empty() {
                    return Ok((Vec::new(), 0));
                }
                clauses.push(format!("id IN ({})", vec!["?"; only.len()].join(", ")));
                args.extend(only.iter().cloned().map(Value::Text));
            }
            let sql_where = if clauses.is_empty() {
                String::new()
            } else {
                format!("WHERE {}", clauses.join(" AND "))
            };
            let total: u64 = c.query_row(
                &format!("SELECT COUNT(*) FROM jobs {sql_where}"),
                rusqlite::params_from_iter(args.iter()),
                |r| r.get(0),
            )?;
            let order = match q.order {
                JobOrder::Newest => "created_at DESC, id",
                JobOrder::Oldest => "created_at, id",
                JobOrder::Largest => {
                    "(SELECT COALESCE(SUM(size), 0) FROM manifest_entries m \
                      WHERE m.job_id = jobs.id AND m.is_dir = 0) DESC, created_at DESC, id"
                }
            };
            args.push(Value::Integer(i64::from(limit)));
            args.push(Value::Integer(i64::from(offset)));
            let sql =
                format!("SELECT {COLUMNS} FROM jobs {sql_where} ORDER BY {order} LIMIT ? OFFSET ?");
            let rows = query_jobs(c, &sql, rusqlite::params_from_iter(args.iter()))?;
            Ok((rows, total))
        })
        .await
    }

    pub async fn set_job_options(&self, id: &str, options: serde_json::Value) -> Result<()> {
        let id = id.to_owned();
        let options = serde_json::to_string(&options)?;
        self.call(move |c| {
            let n = c.execute(
                "UPDATE jobs SET options = ?2, updated_at = ?3 WHERE id = ?1",
                params![id, options, now_ms()],
            )?;
            expect_one(n, &id)
        })
        .await
    }
}

/// A source root of a job that was sent, for "Recently sent from" (docs/design/specs/D02).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecentSourceRow {
    /// As stored: the absolute path's raw bytes.
    pub path: Vec<u8>,
    pub job_id: String,
    pub profile_id: String,
    pub sent_at: i64,
}

impl Store {
    /// Source roots of jobs past `ready` (they were sent), one per path, newest first.
    pub async fn recent_sources(&self, limit: u32) -> Result<Vec<RecentSourceRow>> {
        self.call(move |c| {
            let mut stmt = c.prepare_cached(
                "SELECT s.path, j.id, j.profile_id, MAX(j.created_at) AS at
                 FROM job_sources s JOIN jobs j ON j.id = s.job_id
                 WHERE j.state NOT IN ('draft', 'analysing', 'awaiting_approval', 'ready')
                 GROUP BY s.path ORDER BY at DESC LIMIT ?1",
            )?;
            let rows = stmt.query_map([limit], |r| {
                Ok(RecentSourceRow {
                    path: r.get(0)?,
                    job_id: r.get(1)?,
                    profile_id: r.get(2)?,
                    sent_at: r.get(3)?,
                })
            })?;
            Ok(rows.collect::<rusqlite::Result<_>>()?)
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::StateChange;
    use crate::StoreError;
    use crate::test_util::store_with_job;

    fn change(from: &str, to: &str, error: Option<&str>) -> StateChange {
        StateChange {
            from: from.into(),
            to: to.into(),
            error: error.map(str::to_owned),
            ..StateChange::default()
        }
    }

    #[tokio::test]
    async fn lifecycle_and_resume_list() {
        let (store, id) = store_with_job().await;
        let job = store.get_job(&id).await.unwrap();
        assert_eq!(job.state, "draft");
        assert_eq!(job.sources, vec![b"/home/u/Backup".to_vec()]);
        assert_eq!(job.finished_at, None);

        store
            .change_job_state(&id, change("draft", "retrying", Some("connection reset")))
            .await
            .unwrap();
        store.set_job_retry(&id, 3, Some(123)).await.unwrap();
        let job = store.get_job(&id).await.unwrap();
        assert_eq!(
            (job.state.as_str(), job.attempt, job.next_retry_at),
            ("retrying", 3, Some(123))
        );
        assert_eq!(store.list_unfinished_jobs().await.unwrap().len(), 1);

        let done = StateChange {
            finished: true,
            warnings: vec!["2 files could not be verified".into()],
            verify_round: true,
            ..change("retrying", "completed_with_warnings", None)
        };
        store.change_job_state(&id, done).await.unwrap();
        let job = store.get_job(&id).await.unwrap();
        assert!(job.finished_at.is_some());
        assert_eq!(job.next_retry_at, None);
        assert_eq!(job.warnings, ["2 files could not be verified"]);
        assert_eq!(job.verify_rounds, 1);
        let events = store.events_page(&id, None, 10).await.unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[1].data["to"], "completed_with_warnings");
        assert!(store.list_unfinished_jobs().await.unwrap().is_empty());
        assert_eq!(store.list_jobs(10, 0).await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn source_paths_keep_non_utf8_bytes() {
        let (store, _) = store_with_job().await;
        store
            .insert_job(crate::NewJob {
                id: "j2".into(),
                kind: "transfer".into(),
                profile_id: "p1".into(),
                state: "draft".into(),
                options: serde_json::json!({ "verify": true }),
                sources: vec![b"/a/Patol\xf3gica".to_vec(), b"/b".to_vec()],
                label: None,
                again_of: None,
            })
            .await
            .unwrap();
        let job = store.get_job("j2").await.unwrap();
        assert_eq!(job.sources[0], b"/a/Patol\xf3gica");
        assert_eq!(job.options["verify"], true);
    }

    #[tokio::test]
    async fn unknown_job_is_not_found() {
        let (store, _) = store_with_job().await;
        assert!(matches!(
            store
                .change_job_state("nope", change("draft", "x", None))
                .await,
            Err(StoreError::NotFound(_))
        ));
    }

    #[tokio::test]
    async fn a_state_change_from_a_stale_state_is_refused() {
        let (store, id) = store_with_job().await;
        assert!(matches!(
            store
                .change_job_state(&id, change("ready", "transferring", None))
                .await,
            Err(StoreError::Invalid(_))
        ));
        assert_eq!(store.get_job(&id).await.unwrap().state, "draft");
        assert!(store.events_page(&id, None, 10).await.unwrap().is_empty());
    }
}
