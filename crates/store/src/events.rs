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
    }
}
