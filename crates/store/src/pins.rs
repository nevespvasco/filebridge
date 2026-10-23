//! Pinned sources: folders and files the user keeps at hand in step 1 (docs/design/specs/D02).

use rusqlite::params;

use crate::{Result, Store, now_ms};

/// A pinned folder or file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PinnedSourceRow {
    pub path: String,
    pub pinned_at: i64,
}

impl Store {
    /// The pinned sources, in the order they were pinned.
    pub async fn pinned_sources(&self) -> Result<Vec<PinnedSourceRow>> {
        self.call(|c| {
            let mut stmt =
                c.prepare_cached("SELECT path, pinned_at FROM pinned_sources ORDER BY rowid")?;
            let rows = stmt.query_map([], |r| {
                Ok(PinnedSourceRow {
                    path: r.get(0)?,
                    pinned_at: r.get(1)?,
                })
            })?;
            Ok(rows.collect::<rusqlite::Result<_>>()?)
        })
        .await
    }

    /// Pins `path` (keeping its place if it already is), or unpins it. Answers how many are
    /// pinned after.
    pub async fn set_pinned(&self, path: &str, pinned: bool) -> Result<u32> {
        let path = path.to_owned();
        self.call(move |c| {
            if pinned {
                c.execute(
                    "INSERT INTO pinned_sources (path, pinned_at) VALUES (?1, ?2)
                     ON CONFLICT(path) DO NOTHING",
                    params![path, now_ms()],
                )?;
            } else {
                c.execute("DELETE FROM pinned_sources WHERE path = ?1", [&path])?;
            }
            Ok(c.query_row("SELECT COUNT(*) FROM pinned_sources", [], |r| r.get(0))?)
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use crate::Store;

    #[tokio::test]
    async fn pins_keep_their_order_and_are_kept_once() {
        let store = Store::open_in_memory().unwrap();
        assert!(store.pinned_sources().await.unwrap().is_empty());
        assert_eq!(store.set_pinned("/home/demo/Fotos", true).await.unwrap(), 1);
        assert_eq!(
            store.set_pinned("/home/demo/Música", true).await.unwrap(),
            2
        );
        let first = store.pinned_sources().await.unwrap()[0].clone();
        // Pinning again changes nothing, not even its place.
        assert_eq!(store.set_pinned("/home/demo/Fotos", true).await.unwrap(), 2);
        let pins = store.pinned_sources().await.unwrap();
        assert_eq!(pins[0], first);
        let paths: Vec<_> = pins.iter().map(|p| p.path.as_str()).collect();
        assert_eq!(paths, ["/home/demo/Fotos", "/home/demo/Música"]);

        assert_eq!(
            store.set_pinned("/home/demo/Fotos", false).await.unwrap(),
            1
        );
        assert_eq!(store.set_pinned("/nope", false).await.unwrap(), 1);
        let paths: Vec<_> = store
            .pinned_sources()
            .await
            .unwrap()
            .into_iter()
            .map(|p| p.path)
            .collect();
        assert_eq!(paths, ["/home/demo/Música"]);
    }
}
