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
    pub async fn upsert_profile(&self, p: ProfileRow) -> Result<()> {
        let config = serde_json::to_string(&p.config)?;
        self.call(move |c| {
            let now = now_ms();
            c.execute(
                "INSERT INTO profiles (id, name, transport, integration, config, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6)
                 ON CONFLICT(id) DO UPDATE SET name = ?2, transport = ?3, integration = ?4,
                     config = ?5, updated_at = ?6",
                params![p.id, p.name, p.transport, p.integration, config, now],
            )?;
            Ok(())
        })
        .await
    }

    pub async fn get_profile(&self, id: &str) -> Result<ProfileRow> {
        let id = id.to_owned();
        self.call(move |c| {
            let pair = c
                .query_row(
                    &format!("SELECT {COLUMNS} FROM profiles WHERE id = ?1"),
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
            let mut stmt = c.prepare(&format!("SELECT {COLUMNS} FROM profiles ORDER BY name"))?;
            let rows = stmt.query_map([], ProfileRow::from_row)?;
            rows.map(|r| ProfileRow::parse(r?)).collect()
        })
        .await
    }

    /// Fails while any job still references the profile.
    pub async fn delete_profile(&self, id: &str) -> Result<()> {
        let id = id.to_owned();
        self.call(move |c| {
            let n = c.execute("DELETE FROM profiles WHERE id = ?1", [&id])?;
            if n == 0 {
                return Err(StoreError::NotFound(format!("profile {id}")));
            }
            Ok(())
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
    async fn profile_in_use_cannot_be_deleted() {
        let (store, _) = store_with_job().await;
        assert!(matches!(
            store.delete_profile("p1").await,
            Err(StoreError::Sqlite(_))
        ));
    }
}
