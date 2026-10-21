use rusqlite::{Connection, OptionalExtension, Row, params};

use crate::{Result, Store, StoreError, now_ms};

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
}

const COLUMNS: &str = "id, kind, profile_id, state, options, error, attempt, next_retry_at, \
                       created_at, updated_at, finished_at";

fn from_row(row: &Row<'_>) -> rusqlite::Result<(JobRow, String)> {
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
        },
        row.get(4)?,
    ))
}

fn load(c: &Connection, pair: (JobRow, String)) -> Result<JobRow> {
    let (mut job, options) = pair;
    job.options = serde_json::from_str(&options)?;
    let mut stmt =
        c.prepare_cached("SELECT path FROM job_sources WHERE job_id = ?1 ORDER BY position")?;
    job.sources = stmt
        .query_map([&job.id], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    Ok(job)
}

fn query_jobs(c: &Connection, sql: &str, params: impl rusqlite::Params) -> Result<Vec<JobRow>> {
    let mut stmt = c.prepare(sql)?;
    let pairs = stmt
        .query_map(params, from_row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    pairs.into_iter().map(|p| load(c, p)).collect()
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
                "INSERT INTO jobs (id, kind, profile_id, state, options, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6)",
                params![job.id, job.kind, job.profile_id, job.state, options, now],
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
            let pair = c
                .query_row(
                    &format!("SELECT {COLUMNS} FROM jobs WHERE id = ?1"),
                    [&id],
                    from_row,
                )
                .optional()?
                .ok_or_else(|| StoreError::NotFound(format!("job {id}")))?;
            load(c, pair)
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

    /// Records a state transition. `error` replaces the previous one (`None` clears it).
    pub async fn set_job_state(&self, id: &str, state: &str, error: Option<String>) -> Result<()> {
        let (id, state) = (id.to_owned(), state.to_owned());
        self.call(move |c| {
            let n = c.execute(
                "UPDATE jobs SET state = ?2, error = ?3, updated_at = ?4 WHERE id = ?1",
                params![id, state, error, now_ms()],
            )?;
            expect_one(n, &id)
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

    /// Moves the job to a terminal state; it will no longer be resumed.
    pub async fn finish_job(&self, id: &str, state: &str, error: Option<String>) -> Result<()> {
        let (id, state) = (id.to_owned(), state.to_owned());
        self.call(move |c| {
            let now = now_ms();
            let n = c.execute(
                "UPDATE jobs SET state = ?2, error = ?3, updated_at = ?4, finished_at = ?4,
                     next_retry_at = NULL
                 WHERE id = ?1",
                params![id, state, error, now],
            )?;
            expect_one(n, &id)
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use crate::StoreError;
    use crate::test_util::store_with_job;

    #[tokio::test]
    async fn lifecycle_and_resume_list() {
        let (store, id) = store_with_job().await;
        let job = store.get_job(&id).await.unwrap();
        assert_eq!(job.state, "draft");
        assert_eq!(job.sources, vec![b"/home/u/Backup".to_vec()]);
        assert_eq!(job.finished_at, None);

        store
            .set_job_state(&id, "retrying", Some("connection reset".into()))
            .await
            .unwrap();
        store.set_job_retry(&id, 3, Some(123)).await.unwrap();
        let job = store.get_job(&id).await.unwrap();
        assert_eq!(
            (job.state.as_str(), job.attempt, job.next_retry_at),
            ("retrying", 3, Some(123))
        );
        assert_eq!(store.list_unfinished_jobs().await.unwrap().len(), 1);

        store.finish_job(&id, "completed", None).await.unwrap();
        let job = store.get_job(&id).await.unwrap();
        assert!(job.finished_at.is_some());
        assert_eq!(job.next_retry_at, None);
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
            store.set_job_state("nope", "x", None).await,
            Err(StoreError::NotFound(_))
        ));
    }
}
