use rusqlite::{OptionalExtension, Row, params};

use crate::{Result, Store, StoreError, now_ms};

/// A saved destination. `config` is JSON owned by `core`/the plugins and must hold only keyring
/// references, never secrets.
#[derive(Debug, Clone, PartialEq)]
pub struct ProfileRow {
    pub id: String,
    pub name: String,
    pub transport: String,
    pub integration: Option<String>,
    pub config: serde_json::Value,
    pub created_at: i64,
    pub updated_at: i64,
}

impl ProfileRow {
    fn from_row(row: &Row<'_>) -> rusqlite::Result<(Self, String)> {
        Ok((
            Self {
                id: row.get(0)?,
                name: row.get(1)?,
                transport: row.get(2)?,
                integration: row.get(3)?,
                config: serde_json::Value::Null,
                created_at: row.get(5)?,
                updated_at: row.get(6)?,
            },
            row.get(4)?,
        ))
    }

    fn parse(pair: (Self, String)) -> Result<Self> {
        let (mut p, config) = pair;
        p.config = serde_json::from_str(&config)?;
        Ok(p)
    }
}

const COLUMNS: &str = "id, name, transport, integration, config, created_at, updated_at";

impl Store {
    /// Inserts or updates a profile. `created_at` is kept on update; `updated_at` is set to now.
    /// A deleted profile is not brought back: `Deleted`, and nothing is written.
    pub async fn upsert_profile(&self, p: ProfileRow) -> Result<()> {
        let config = serde_json::to_string(&p.config)?;
        self.call(move |c| {
            let now = now_ms();
            let n = c.execute(
                "INSERT INTO profiles (id, name, transport, integration, config, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6)
                 ON CONFLICT(id) DO UPDATE SET name = ?2, transport = ?3, integration = ?4,
                     config = ?5, updated_at = ?6
                 WHERE profiles.deleted_at IS NULL",
                params![p.id, p.name, p.transport, p.integration, config, now],
            )?;
            if n == 0 {
                return Err(StoreError::Deleted(format!("profile {}", p.id)));
            }
            Ok(())
        })
        .await
    }

    /// A profile that can be used: not deleted.
    pub async fn get_profile(&self, id: &str) -> Result<ProfileRow> {
        self.profile_where(id, "AND deleted_at IS NULL").await
    }

    /// A profile for the history of the jobs that used it (their report, a file's details,
    /// the activity), deleted or not. Never for sending.
    pub async fn get_profile_for_history(&self, id: &str) -> Result<ProfileRow> {
        self.profile_where(id, "").await
    }

    async fn profile_where(&self, id: &str, and: &'static str) -> Result<ProfileRow> {
        let id = id.to_owned();
        self.call(move |c| {
            let pair = c
                .query_row(
                    &format!("SELECT {COLUMNS} FROM profiles WHERE id = ?1 {and}"),
                    [&id],
                    ProfileRow::from_row,
                )
                .optional()?
                .ok_or_else(|| StoreError::NotFound(format!("profile {id}")))?;
            ProfileRow::parse(pair)
        })
        .await
    }

    pub async fn list_profiles(&self) -> Result<Vec<ProfileRow>> {
        self.call(|c| {
            let mut stmt = c.prepare(&format!(
                "SELECT {COLUMNS} FROM profiles WHERE deleted_at IS NULL ORDER BY name"
            ))?;
            let rows = stmt.query_map([], ProfileRow::from_row)?;
            rows.map(|r| ProfileRow::parse(r?)).collect()
        })
        .await
    }

    /// Forgets a profile: gone from every list and lookup, so nothing can be sent to it again.
    /// Its row stays (marked deleted), because the jobs that used it keep their history.
    pub async fn delete_profile(&self, id: &str) -> Result<()> {
        let id = id.to_owned();
        self.call(move |c| {
            let n = c.execute(
                "UPDATE profiles SET deleted_at = ?2 WHERE id = ?1 AND deleted_at IS NULL",
                params![id, now_ms()],
            )?;
            if n == 0 {
                return Err(StoreError::NotFound(format!("profile {id}")));
            }
            Ok(())
        })
        .await
    }
}

impl Store {
    /// Remembers the folder a transfer to this profile was sent into (`None` = its root).
    pub async fn set_last_target_dir(&self, id: &str, dir: Option<String>) -> Result<()> {
        let id = id.to_owned();
        self.call(move |c| {
            let n = c.execute(
                "UPDATE profiles SET last_target_dir = ?2 WHERE id = ?1 AND deleted_at IS NULL",
                params![id, dir],
            )?;
            if n == 0 {
                return Err(StoreError::NotFound(format!("profile {id}")));
            }
            Ok(())
        })
        .await
    }

    /// The folder the last transfer to this profile was sent into (`None` = its root, or none
    /// was sent yet).
    pub async fn last_target_dir(&self, id: &str) -> Result<Option<String>> {
        let id = id.to_owned();
        self.call(move |c| {
            c.query_row(
                "SELECT last_target_dir FROM profiles WHERE id = ?1 AND deleted_at IS NULL",
                [&id],
                |r| r.get(0),
            )
            .optional()?
            .ok_or_else(|| StoreError::NotFound(format!("profile {id}")))
        })
        .await
    }
}

/// `last_test, last_test_at, last_good_test, last_good_test_at`.
type TestColumns = (Option<String>, Option<i64>, Option<String>, Option<i64>);

/// A profile's last connection test, and its last one that reached the destination.
#[derive(Debug, Clone, PartialEq)]
pub struct StoredTest {
    pub last: Option<(serde_json::Value, i64)>,
    pub last_good: Option<(serde_json::Value, i64)>,
}

impl Store {
    /// Records a connection test (`ok`: the destination answered, so its numbers are good).
    pub async fn save_profile_test(
        &self,
        id: &str,
        test: serde_json::Value,
        ok: bool,
    ) -> Result<()> {
        let (id, test) = (id.to_owned(), serde_json::to_string(&test)?);
        self.call(move |c| {
            let now = now_ms();
            let n = if ok {
                c.execute(
                    "UPDATE profiles SET last_test = ?2, last_test_at = ?3,
                         last_good_test = ?2, last_good_test_at = ?3 WHERE id = ?1",
                    params![id, test, now],
                )?
            } else {
                c.execute(
                    "UPDATE profiles SET last_test = ?2, last_test_at = ?3 WHERE id = ?1",
                    params![id, test, now],
                )?
            };
            if n == 0 {
                return Err(StoreError::NotFound(format!("profile {id}")));
            }
            Ok(())
        })
        .await
    }

    pub async fn profile_test(&self, id: &str) -> Result<StoredTest> {
        let id = id.to_owned();
        self.call(move |c| {
            let row: Option<TestColumns> = c
                .query_row(
                    "SELECT last_test, last_test_at, last_good_test, last_good_test_at
                     FROM profiles WHERE id = ?1",
                    [&id],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
                )
                .optional()?;
            let Some((last, last_at, good, good_at)) = row else {
                return Err(StoreError::NotFound(format!("profile {id}")));
            };
            let pair =
                |v: Option<String>, at: Option<i64>| -> Result<Option<(serde_json::Value, i64)>> {
                    match (v, at) {
                        (Some(v), Some(at)) => Ok(Some((serde_json::from_str(&v)?, at))),
                        _ => Ok(None),
                    }
                };
            Ok(StoredTest {
                last: pair(last, last_at)?,
                last_good: pair(good, good_at)?,
            })
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::store_with_job;

    #[tokio::test]
    async fn upsert_get_list() {
        let (store, _) = store_with_job().await;
        let mut p = store.get_profile("p1").await.unwrap();
        assert_eq!(p.config["root"], "/data");
        let created = p.created_at;
        p.name = "Renamed".into();
        store.upsert_profile(p).await.unwrap();
        let p = store.get_profile("p1").await.unwrap();
        assert_eq!(p.name, "Renamed");
        assert_eq!(p.created_at, created);
        assert_eq!(store.list_profiles().await.unwrap().len(), 1);
        assert!(matches!(
            store.get_profile("nope").await,
            Err(StoreError::NotFound(_))
        ));
    }

    #[tokio::test]
    async fn last_target_dir_is_remembered_and_kept_on_save() {
        let (store, _) = store_with_job().await;
        assert_eq!(store.last_target_dir("p1").await.unwrap(), None);
        store
            .set_last_target_dir("p1", Some("2019/Fotos".into()))
            .await
            .unwrap();
        // Editing the profile doesn't forget it.
        let p = store.get_profile("p1").await.unwrap();
        store.upsert_profile(p).await.unwrap();
        assert_eq!(
            store.last_target_dir("p1").await.unwrap().as_deref(),
            Some("2019/Fotos")
        );
        store.set_last_target_dir("p1", None).await.unwrap();
        assert_eq!(store.last_target_dir("p1").await.unwrap(), None);
        assert!(matches!(
            store.last_target_dir("nope").await,
            Err(StoreError::NotFound(_))
        ));
        store.delete_profile("p1").await.unwrap();
        assert!(store.set_last_target_dir("p1", None).await.is_err());
    }

    #[tokio::test]
    async fn a_used_profile_is_forgotten_and_its_jobs_stay() {
        let (store, job) = store_with_job().await;
        let unfinished = store.unfinished_jobs_of_profile("p1").await.unwrap();
        assert_eq!(unfinished.len(), 1, "the draft counts");
        assert!(
            store
                .unfinished_jobs_of_profile("p2")
                .await
                .unwrap()
                .is_empty()
        );
        store.delete_profile("p1").await.unwrap();
        assert!(matches!(
            store.get_profile("p1").await,
            Err(StoreError::NotFound(_))
        ));
        assert!(store.list_profiles().await.unwrap().is_empty());
        assert_eq!(store.get_job(&job).await.unwrap().profile_id, "p1");
        assert!(matches!(
            store.delete_profile("p1").await,
            Err(StoreError::NotFound(_))
        ));
    }

    #[tokio::test]
    async fn a_deleted_profile_stays_readable_for_history_only() {
        let (store, _) = store_with_job().await;
        let before = store.get_profile("p1").await.unwrap();
        store.delete_profile("p1").await.unwrap();
        let kept = store.get_profile_for_history("p1").await.unwrap();
        assert_eq!(kept.name, before.name);
        assert!(matches!(
            store.get_profile_for_history("nope").await,
            Err(StoreError::NotFound(_))
        ));
        // Saving it again doesn't quietly update a row nobody can see.
        let err = store.upsert_profile(before.clone()).await.unwrap_err();
        assert!(matches!(err, StoreError::Deleted(_)), "{err}");
        assert!(store.get_profile("p1").await.is_err());
        assert_eq!(
            store
                .get_profile_for_history("p1")
                .await
                .unwrap()
                .updated_at,
            kept.updated_at
        );
    }
}
