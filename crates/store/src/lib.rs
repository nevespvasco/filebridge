//! SQLite persistence: jobs, manifests, profiles, issues, events and settings.
//!
//! One dedicated thread owns the connection; every call is a closure sent to it, so writes are
//! serialised and async callers never block the runtime. Rows here are plain data: `core` owns
//! the domain types (job states, pipeline stages) and stores them as text.
//!
//! Never store secrets here. Profiles hold only keyring references (see ADR 0004).

mod events;
mod issues;
mod jobs;
mod manifest;
mod profiles;
mod schema;
mod settings;

use std::path::Path;
use std::sync::mpsc as std_mpsc;
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::Connection;
use tokio::sync::oneshot;

pub use events::EventRow;
pub use issues::{Decision, IssueRow, NewIssue, RuleSummary};
pub use jobs::{JobRow, NewJob, RecentSourceRow, StateChange};
pub use manifest::{EntryState, ManifestCounts, ManifestEntry, NewEntry};
pub use profiles::ProfileRow;

pub type Result<T, E = StoreError> = std::result::Result<T, E>;

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("database error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("invalid JSON in the database: {0}")]
    Json(#[from] serde_json::Error),
    #[error("{0} not found")]
    NotFound(String),
    #[error("invalid value in the database: {0}")]
    Invalid(String),
    #[error("the store thread has stopped")]
    Closed,
}

type Task = Box<dyn FnOnce(&mut Connection) + Send>;

/// Handle to the database. Cheap to clone; all clones share the same connection thread.
#[derive(Clone)]
pub struct Store {
    tx: std_mpsc::Sender<Task>,
}

impl Store {
    /// Opens (or creates) the database file and runs pending migrations.
    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        Self::start(conn)
    }

    /// A private in-memory database, for tests.
    pub fn open_in_memory() -> Result<Self> {
        Self::start(Connection::open_in_memory()?)
    }

    fn start(mut conn: Connection) -> Result<Self> {
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        schema::migrate(&mut conn)?;
        let (tx, rx) = std_mpsc::channel::<Task>();
        thread::Builder::new()
            .name("filebridge-store".into())
            .spawn(move || {
                while let Ok(task) = rx.recv() {
                    task(&mut conn);
                }
            })
            .map_err(|e| StoreError::Invalid(format!("cannot start store thread: {e}")))?;
        Ok(Self { tx })
    }

    /// Runs `f` on the connection thread and returns its result.
    pub async fn call<R, F>(&self, f: F) -> Result<R>
    where
        R: Send + 'static,
        F: FnOnce(&mut Connection) -> Result<R> + Send + 'static,
    {
        let (reply, rx) = oneshot::channel();
        self.tx
            .send(Box::new(move |conn| {
                let _ = reply.send(f(conn));
            }))
            .map_err(|_| StoreError::Closed)?;
        rx.await.map_err(|_| StoreError::Closed)?
    }
}

/// Milliseconds since the Unix epoch.
pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

#[cfg(test)]
pub(crate) mod test_util {
    use super::*;

    pub async fn store_with_job() -> (Store, String) {
        let store = Store::open_in_memory().unwrap();
        store
            .upsert_profile(ProfileRow {
                id: "p1".into(),
                name: "Nextcloud home".into(),
                transport: "rclone".into(),
                integration: Some("nextcloud".into()),
                config: serde_json::json!({ "root": "/data" }),
                created_at: 0,
                updated_at: 0,
            })
            .await
            .unwrap();
        store
            .insert_job(NewJob {
                id: "j1".into(),
                kind: "transfer".into(),
                profile_id: "p1".into(),
                state: "draft".into(),
                options: serde_json::json!({}),
                sources: vec![b"/home/u/Backup".to_vec()],
            })
            .await
            .unwrap();
        (store, "j1".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn file_database_survives_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("filebridge.db");
        {
            let store = Store::open(&path).unwrap();
            store.set_setting("language", "\"pt\"").await.unwrap();
        }
        let store = Store::open(&path).unwrap();
        assert_eq!(
            store.get_setting("language").await.unwrap().as_deref(),
            Some("\"pt\"")
        );
    }

    #[tokio::test]
    async fn clones_share_the_connection() {
        let store = Store::open_in_memory().unwrap();
        let other = store.clone();
        store.set_setting("k", "1").await.unwrap();
        assert_eq!(other.get_setting("k").await.unwrap().as_deref(), Some("1"));
    }
}
