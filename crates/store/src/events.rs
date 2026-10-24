//! Append-only job log for the Job detail screen. Never put credentials in `data`.

use rusqlite::params;

use crate::{Result, Store, now_ms};

#[derive(Debug, Clone, PartialEq)]
pub struct EventRow {
    pub id: i64,
    pub job_id: Option<String>,
    pub at: i64,
    pub kind: String,
    pub data: serde_json::Value,
}

impl Store {
    /// Appends an event; `job_id` is `None` for app-level events. Returns the event id.
    pub async fn append_event(
        &self,
        job_id: Option<&str>,
        kind: &str,
        data: serde_json::Value,
    ) -> Result<i64> {
        let (job_id, kind) = (job_id.map(str::to_owned), kind.to_owned());
        let data = serde_json::to_string(&data)?;
        self.call(move |c| {
            c.execute(
                "INSERT INTO events (job_id, at, kind, data) VALUES (?1, ?2, ?3, ?4)",
                params![job_id, now_ms(), kind, data],
            )?;
            Ok(c.last_insert_rowid())
        })
        .await
    }

    /// A job's events in order, after `after_id`.
    pub async fn events_page(
        &self,
        job_id: &str,
        after_id: Option<i64>,
        limit: u32,
    ) -> Result<Vec<EventRow>> {
        let job_id = job_id.to_owned();
        self.call(move |c| {
            let mut stmt = c.prepare_cached(
                "SELECT id, job_id, at, kind, data FROM events
                 WHERE job_id = ?1 AND (?2 IS NULL OR id > ?2) ORDER BY id LIMIT ?3",
            )?;
            let rows = stmt.query_map(params![job_id, after_id, limit], |r| {
                Ok((
                    EventRow {
                        id: r.get(0)?,
                        job_id: r.get(1)?,
                        at: r.get(2)?,
                        kind: r.get(3)?,
                        data: serde_json::Value::Null,
                    },
                    r.get::<_, String>(4)?,
                ))
            })?;
            rows.map(|r| {
                let (mut e, data) = r?;
                e.data = serde_json::from_str(&data)?;
                Ok(e)
            })
            .collect()
        })
        .await
    }
}

impl Store {
    /// A job's events newest first, before `before_id` (the newest when `None`).
    pub async fn events_page_before(
        &self,
        job_id: &str,
        before_id: Option<i64>,
        limit: u32,
    ) -> Result<Vec<EventRow>> {
        let job_id = job_id.to_owned();
        self.call(move |c| {
            let mut stmt = c.prepare_cached(
                "SELECT id, job_id, at, kind, data FROM events
                 WHERE job_id = ?1 AND (?2 IS NULL OR id < ?2) ORDER BY id DESC LIMIT ?3",
            )?;
            let rows = stmt.query_map(params![job_id, before_id, limit], |r| {
                Ok((
                    EventRow {
                        id: r.get(0)?,
                        job_id: r.get(1)?,
                        at: r.get(2)?,
                        kind: r.get(3)?,
                        data: serde_json::Value::Null,
                    },
                    r.get::<_, String>(4)?,
                ))
            })?;
            rows.map(|r| {
                let (mut e, data) = r?;
                e.data = serde_json::from_str(&data)?;
                Ok(e)
            })
            .collect()
        })
        .await
    }
}

impl Store {
    /// Milestones of every job and profile changes, newest first, before `before_id`
    /// (docs/design/specs/D07, "Activity"): created, the first start, the first retry or wait
    /// of a run (repeats collapse), finished, and profile added/removed. Nothing of a discarded
    /// draft (ADR 0017).
    pub async fn activity_page(&self, before_id: Option<i64>, limit: u32) -> Result<Vec<EventRow>> {
        self.call(move |c| {
            let mut stmt = c.prepare_cached(
                "WITH m AS (
                     SELECT id, job_id, at, kind, data,
                            ROW_NUMBER() OVER (PARTITION BY job_id, kind ORDER BY id) AS nth
                     FROM events
                     WHERE (kind IN ('created', 'retry', 'send_window_wait', 'finished', 'profile')
                            OR (kind = 'state' AND json_extract(data, '$.to') = 'transferring'))
                       AND (job_id IS NULL
                            OR job_id NOT IN (SELECT id FROM jobs WHERE discarded_at IS NOT NULL))
                 ),
                 firsts AS (SELECT * FROM m WHERE NOT (kind = 'state' AND nth > 1)),
                 r AS (
                     SELECT *, LAG(kind) OVER (PARTITION BY job_id ORDER BY id) AS prev
                     FROM firsts
                 )
                 SELECT id, job_id, at, kind, data FROM r
                 WHERE NOT (kind IN ('retry', 'send_window_wait') AND prev = kind)
                   AND (?1 IS NULL OR id < ?1)
                 ORDER BY id DESC LIMIT ?2",
            )?;
            let rows = stmt.query_map(params![before_id, limit], |r| {
                Ok((
                    EventRow {
                        id: r.get(0)?,
                        job_id: r.get(1)?,
                        at: r.get(2)?,
                        kind: r.get(3)?,
                        data: serde_json::Value::Null,
                    },
                    r.get::<_, String>(4)?,
                ))
            })?;
            rows.map(|r| {
                let (mut e, data) = r?;
                e.data = serde_json::from_str(&data)?;
                Ok(e)
            })
            .collect()
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use crate::test_util::store_with_job;

    #[tokio::test]
    async fn append_and_page() {
        let (store, job) = store_with_job().await;
        let first = store
            .append_event(
                Some(&job),
                "state",
                serde_json::json!({ "to": "analysing" }),
            )
            .await
            .unwrap();
        store
            .append_event(Some(&job), "state", serde_json::json!({ "to": "ready" }))
            .await
            .unwrap();
        store
            .append_event(None, "app", serde_json::json!({ "started": true }))
            .await
            .unwrap();
        let all = store.events_page(&job, None, 10).await.unwrap();
        assert_eq!(all.len(), 2);
        assert_eq!(all[1].data["to"], "ready");
        let after = store.events_page(&job, Some(first), 10).await.unwrap();
        assert_eq!(after.len(), 1);
        let activity = store.activity_page(None, 10).await.unwrap();
        assert!(activity.is_empty(), "no milestone kinds yet");
        let newest = store.events_page_before(&job, None, 10).await.unwrap();
        assert_eq!(
            newest.iter().map(|e| e.id).collect::<Vec<_>>(),
            [all[1].id, all[0].id]
        );
        let older = store
            .events_page_before(&job, Some(all[1].id), 10)
            .await
            .unwrap();
        assert_eq!(older.len(), 1);
        assert_eq!(older[0].id, first);
    }

    #[tokio::test]
    async fn activity_keeps_milestones_and_collapses_repeats() {
        use serde_json::json;
        let (store, job) = store_with_job().await;
        let add = |kind: &'static str, data: serde_json::Value| {
            let (store, job) = (store.clone(), job.clone());
            async move { store.append_event(Some(&job), kind, data).await.unwrap() }
        };
        add("created", json!({})).await;
        add("state", json!({ "to": "transferring" })).await;
        add("retry", json!({ "attempt": 1 })).await;
        add("state", json!({ "to": "retrying" })).await;
        add("state", json!({ "to": "transferring" })).await;
        add("retry", json!({ "attempt": 2 })).await;
        add("retry", json!({ "attempt": 3 })).await;
        add("transfer_round", json!({ "sent": 3 })).await;
        add("finished", json!({ "state": "completed" })).await;
        store
            .append_event(None, "profile", json!({ "action": "added" }))
            .await
            .unwrap();
        let kinds: Vec<_> = store
            .activity_page(None, 20)
            .await
            .unwrap()
            .into_iter()
            .map(|e| e.kind)
            .collect();
        assert_eq!(kinds, ["profile", "finished", "retry", "state", "created"]);
        let older = store.activity_page(Some(i64::MAX), 2).await.unwrap();
        assert_eq!(older.len(), 2);
    }
}
