//! The manifest: one row per scanned file or directory, with the raw source path, the corrected
//! destination path and per-entry transfer and verify state. Verification works from here, so it
//! does not depend on the final name (ADR 0005).

use rusqlite::{Row, params};

use crate::{Result, Store, StoreError};

/// Where an entry is in transfer or verification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EntryState {
    Pending,
    /// Transfer started but did not finish (resumable).
    Partial,
    Done,
    Failed,
    /// Left out by a user decision (e.g. "skip file" on a name issue).
    Skipped,
    /// Verification found the destination different from the manifest.
    Mismatch,
    /// Verification found nothing at the destination path.
    Missing,
}

impl EntryState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Partial => "partial",
            Self::Done => "done",
            Self::Failed => "failed",
            Self::Skipped => "skipped",
            Self::Mismatch => "mismatch",
            Self::Missing => "missing",
        }
    }

    pub fn parse(s: &str) -> Result<Self> {
        Ok(match s {
            "pending" => Self::Pending,
            "partial" => Self::Partial,
            "done" => Self::Done,
            "failed" => Self::Failed,
            "skipped" => Self::Skipped,
            "mismatch" => Self::Mismatch,
            "missing" => Self::Missing,
            other => return Err(StoreError::Invalid(format!("entry state {other:?}"))),
        })
    }
}

/// A scanned entry, in scanner order (parents before children).
#[derive(Debug, Clone, PartialEq)]
pub struct NewEntry {
    pub idx: u64,
    /// Index into the job's sources.
    pub source: u32,
    /// Relative to its source root, as raw bytes.
    pub raw_path: Vec<u8>,
    pub parent_idx: Option<u64>,
    pub is_dir: bool,
    pub size: u64,
    pub mtime: Option<i64>,
    pub hash: Option<Vec<u8>>,
    pub hash_algo: Option<String>,
    /// The file's Category (`image`, `video`, …; `other` for directories).
    pub category: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ManifestEntry {
    pub entry: NewEntry,
    /// Destination-relative path after name correction; `None` until stage 3, or when the name
    /// could not be mapped.
    pub dest_path: Option<String>,
    pub transfer_state: EntryState,
    pub verify_state: EntryState,
    pub error: Option<String>,
}

/// Totals for one job's manifest (files only, except `dirs`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ManifestCounts {
    pub files: u64,
    pub dirs: u64,
    pub bytes: u64,
    pub pending: u64,
    pub partial: u64,
    pub done: u64,
    pub failed: u64,
    pub skipped: u64,
    pub bytes_done: u64,
    pub verified_ok: u64,
    pub mismatch: u64,
    pub missing: u64,
}

const COLUMNS: &str = "idx, source, raw_path, parent_idx, is_dir, size, mtime, hash, hash_algo, \
                       dest_path, transfer_state, verify_state, error, category";

fn from_row(row: &Row<'_>) -> rusqlite::Result<(ManifestEntry, String, String)> {
    Ok((
        ManifestEntry {
            entry: NewEntry {
                idx: row.get(0)?,
                source: row.get(1)?,
                raw_path: row.get(2)?,
                parent_idx: row.get(3)?,
                is_dir: row.get(4)?,
                size: row.get(5)?,
                mtime: row.get(6)?,
                hash: row.get(7)?,
                hash_algo: row.get(8)?,
                category: row.get(13)?,
            },
            dest_path: row.get(9)?,
            transfer_state: EntryState::Pending,
            verify_state: EntryState::Pending,
            error: row.get(12)?,
        },
        row.get(10)?,
        row.get(11)?,
    ))
}

fn parse(t: (ManifestEntry, String, String)) -> Result<ManifestEntry> {
    let (mut e, transfer, verify) = t;
    e.transfer_state = EntryState::parse(&transfer)?;
    e.verify_state = EntryState::parse(&verify)?;
    Ok(e)
}

impl Store {
    /// Inserts a batch of entries in one transaction. Callers stream large scans in batches
    /// (e.g. 10 000 at a time).
    pub async fn insert_entries(&self, job_id: &str, entries: Vec<NewEntry>) -> Result<()> {
        let job_id = job_id.to_owned();
        self.call(move |c| {
            let tx = c.transaction()?;
            {
                let mut stmt = tx.prepare_cached(
                    "INSERT INTO manifest_entries
                         (job_id, idx, source, raw_path, parent_idx, is_dir, size, mtime, hash, hash_algo,
                          category)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                )?;
                for e in &entries {
                    stmt.execute(params![
                        job_id, e.idx, e.source, e.raw_path, e.parent_idx, e.is_dir, e.size,
                        e.mtime, e.hash, e.hash_algo, e.category
                    ])?;
                }
            }
            tx.commit()?;
            Ok(())
        })
        .await
    }

    /// Drops a job's manifest, before re-running the analysis.
    pub async fn clear_manifest(&self, job_id: &str) -> Result<()> {
        let job_id = job_id.to_owned();
        self.call(move |c| {
            c.execute("DELETE FROM manifest_entries WHERE job_id = ?1", [&job_id])?;
            Ok(())
        })
        .await
    }

    /// Stores the stage-3 result: `(idx, destination path)`.
    pub async fn set_dest_paths(
        &self,
        job_id: &str,
        paths: Vec<(u64, Option<String>)>,
    ) -> Result<()> {
        let job_id = job_id.to_owned();
        self.call(move |c| {
            let tx = c.transaction()?;
            {
                let mut stmt = tx.prepare_cached(
                    "UPDATE manifest_entries SET dest_path = ?3 WHERE job_id = ?1 AND idx = ?2",
                )?;
                for (idx, path) in &paths {
                    stmt.execute(params![job_id, idx, path])?;
                }
            }
            tx.commit()?;
            Ok(())
        })
        .await
    }

    /// Records transfer outcomes: `(idx, state, error)`.
    pub async fn set_transfer_states(
        &self,
        job_id: &str,
        updates: Vec<(u64, EntryState, Option<String>)>,
    ) -> Result<()> {
        let job_id = job_id.to_owned();
        self.call(move |c| {
            let tx = c.transaction()?;
            {
                let mut stmt = tx.prepare_cached(
                    "UPDATE manifest_entries SET transfer_state = ?3, error = ?4
                     WHERE job_id = ?1 AND idx = ?2",
                )?;
                for (idx, state, error) in &updates {
                    stmt.execute(params![job_id, idx, state.as_str(), error])?;
                }
            }
            tx.commit()?;
            Ok(())
        })
        .await
    }

    /// Moves every entry in one of `from` back to `to` (e.g. partial/failed → pending on
    /// resume, or mismatch → pending to resend only those). Returns how many changed.
    pub async fn reset_transfer_states(
        &self,
        job_id: &str,
        from: &[EntryState],
        to: EntryState,
    ) -> Result<usize> {
        let job_id = job_id.to_owned();
        let from: Vec<&'static str> = from.iter().map(|s| s.as_str()).collect();
        self.call(move |c| {
            let mut n = 0;
            let mut stmt = c.prepare_cached(
                "UPDATE manifest_entries SET transfer_state = ?3, error = NULL
                 WHERE job_id = ?1 AND transfer_state = ?2",
            )?;
            for state in from {
                n += stmt.execute(params![job_id, state, to.as_str()])?;
            }
            Ok(n)
        })
        .await
    }

    /// Records verification outcomes: `(idx, Done | Mismatch | Missing)`.
    pub async fn set_verify_states(
        &self,
        job_id: &str,
        updates: Vec<(u64, EntryState)>,
    ) -> Result<()> {
        let job_id = job_id.to_owned();
        self.call(move |c| {
            let tx = c.transaction()?;
            {
                let mut stmt = tx.prepare_cached(
                    "UPDATE manifest_entries SET verify_state = ?3 WHERE job_id = ?1 AND idx = ?2",
                )?;
                for (idx, state) in &updates {
                    stmt.execute(params![job_id, idx, state.as_str()])?;
                }
            }
            tx.commit()?;
            Ok(())
        })
        .await
    }

    /// Entries in scanner order, after `after_idx` (keyset pagination).
    pub async fn entries_page(
        &self,
        job_id: &str,
        after_idx: Option<u64>,
        limit: u32,
    ) -> Result<Vec<ManifestEntry>> {
        let job_id = job_id.to_owned();
        self.call(move |c| {
            let mut stmt = c.prepare_cached(&format!(
                "SELECT {COLUMNS} FROM manifest_entries
                 WHERE job_id = ?1 AND (?2 IS NULL OR idx > ?2) ORDER BY idx LIMIT ?3"
            ))?;
            let rows = stmt.query_map(params![job_id, after_idx, limit], from_row)?;
            rows.map(|r| parse(r?)).collect()
        })
        .await
    }

    /// Files (not directories) in `state`, in scanner order, after `after_idx`.
    pub async fn files_with_transfer_state(
        &self,
        job_id: &str,
        state: EntryState,
        after_idx: Option<u64>,
        limit: u32,
    ) -> Result<Vec<ManifestEntry>> {
        let job_id = job_id.to_owned();
        self.call(move |c| {
            let mut stmt = c.prepare_cached(&format!(
                "SELECT {COLUMNS} FROM manifest_entries
                 WHERE job_id = ?1 AND is_dir = 0 AND transfer_state = ?2
                   AND (?3 IS NULL OR idx > ?3)
                 ORDER BY idx LIMIT ?4"
            ))?;
            let rows =
                stmt.query_map(params![job_id, state.as_str(), after_idx, limit], from_row)?;
            rows.map(|r| parse(r?)).collect()
        })
        .await
    }

    pub async fn manifest_counts(&self, job_id: &str) -> Result<ManifestCounts> {
        let job_id = job_id.to_owned();
        self.call(move |c| {
            let mut counts = ManifestCounts::default();
            let mut stmt = c.prepare_cached(
                "SELECT is_dir, transfer_state, verify_state, COUNT(*), COALESCE(SUM(size), 0)
                 FROM manifest_entries WHERE job_id = ?1
                 GROUP BY is_dir, transfer_state, verify_state",
            )?;
            let mut rows = stmt.query([&job_id])?;
            while let Some(row) = rows.next()? {
                let is_dir: bool = row.get(0)?;
                let n: u64 = row.get(3)?;
                if is_dir {
                    counts.dirs += n;
                    continue;
                }
                let bytes: u64 = row.get(4)?;
                counts.files += n;
                counts.bytes += bytes;
                match EntryState::parse(&row.get::<_, String>(1)?)? {
                    EntryState::Pending => counts.pending += n,
                    EntryState::Partial => counts.partial += n,
                    EntryState::Done => {
                        counts.done += n;
                        counts.bytes_done += bytes;
                    }
                    EntryState::Failed => counts.failed += n,
                    EntryState::Skipped => counts.skipped += n,
                    EntryState::Mismatch | EntryState::Missing => {}
                }
                match EntryState::parse(&row.get::<_, String>(2)?)? {
                    EntryState::Done => counts.verified_ok += n,
                    EntryState::Mismatch => counts.mismatch += n,
                    EntryState::Missing => counts.missing += n,
                    _ => {}
                }
            }
            Ok(counts)
        })
        .await
    }

    /// Files (not directories) per Category: `(category, files, bytes)`, most bytes first.
    pub async fn category_counts(&self, job_id: &str) -> Result<Vec<(String, u64, u64)>> {
        let job_id = job_id.to_owned();
        self.call(move |c| {
            let mut stmt = c.prepare_cached(
                "SELECT category, COUNT(*), COALESCE(SUM(size), 0) FROM manifest_entries
                 WHERE job_id = ?1 AND is_dir = 0 GROUP BY category ORDER BY 3 DESC, 1",
            )?;
            let rows = stmt.query_map([&job_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
            Ok(rows.collect::<rusqlite::Result<_>>()?)
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::store_with_job;

    fn file(idx: u64, path: &[u8], size: u64) -> NewEntry {
        NewEntry {
            idx,
            source: 0,
            raw_path: path.to_vec(),
            parent_idx: Some(0),
            is_dir: false,
            size,
            mtime: Some(1_700_000_000),
            hash: None,
            hash_algo: None,
            category: if path.ends_with(b".jpg") {
                "image"
            } else {
                "other"
            }
            .into(),
        }
    }

    fn dir(idx: u64, path: &[u8]) -> NewEntry {
        NewEntry {
            is_dir: true,
            parent_idx: None,
            size: 0,
            category: "other".into(),
            ..file(idx, path, 0)
        }
    }

    #[tokio::test]
    async fn counts_files_per_category() {
        let (store, job) = store_with_job().await;
        store
            .insert_entries(
                &job,
                vec![
                    dir(0, b"Fotos"),
                    file(1, b"Fotos/a.jpg", 10),
                    file(2, b"Fotos/b.jpg", 30),
                    file(3, b"notes", 5),
                ],
            )
            .await
            .unwrap();
        let by = store.category_counts(&job).await.unwrap();
        assert_eq!(
            by,
            [("image".into(), 2, 40), ("other".into(), 1, 5)],
            "directories left out"
        );
    }

    #[tokio::test]
    async fn transfer_resume_and_verify_flow() {
        let (store, job) = store_with_job().await;
        // "Fotos/Verão" with an NFD "ã", plus two files.
        store
            .insert_entries(
                &job,
                vec![
                    dir(0, b"Fotos/Vera\xcc\x83o"),
                    file(1, b"Fotos/Vera\xcc\x83o/a.jpg", 100),
                    file(2, b"Fotos/Vera\xcc\x83o/b.jpg", 50),
                ],
            )
            .await
            .unwrap();
        store
            .set_dest_paths(
                &job,
                vec![
                    (0, Some("Fotos/Verão".into())),
                    (1, Some("Fotos/Verão/a.jpg".into())),
                    (2, Some("Fotos/Verão/b.jpg".into())),
                ],
            )
            .await
            .unwrap();

        let pending = store
            .files_with_transfer_state(&job, EntryState::Pending, None, 10)
            .await
            .unwrap();
        assert_eq!(pending.len(), 2);
        assert_eq!(pending[0].entry.raw_path, b"Fotos/Vera\xcc\x83o/a.jpg");
        assert_eq!(pending[0].dest_path.as_deref(), Some("Fotos/Verão/a.jpg"));

        store
            .set_transfer_states(
                &job,
                vec![
                    (1, EntryState::Done, None),
                    (2, EntryState::Partial, Some("connection reset".into())),
                ],
            )
            .await
            .unwrap();
        let c = store.manifest_counts(&job).await.unwrap();
        assert_eq!((c.files, c.dirs, c.bytes), (2, 1, 150));
        assert_eq!((c.done, c.partial, c.bytes_done), (1, 1, 100));

        // Resume: partial goes back to pending, done stays done.
        let n = store
            .reset_transfer_states(
                &job,
                &[EntryState::Partial, EntryState::Failed],
                EntryState::Pending,
            )
            .await
            .unwrap();
        assert_eq!(n, 1);
        let pending = store
            .files_with_transfer_state(&job, EntryState::Pending, None, 10)
            .await
            .unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].entry.idx, 2);
        assert_eq!(pending[0].error, None);

        store
            .set_verify_states(&job, vec![(1, EntryState::Done), (2, EntryState::Mismatch)])
            .await
            .unwrap();
        let c = store.manifest_counts(&job).await.unwrap();
        assert_eq!((c.verified_ok, c.mismatch, c.missing), (1, 1, 0));
    }

    #[tokio::test]
    async fn keyset_pagination() {
        let (store, job) = store_with_job().await;
        let entries = (0..25)
            .map(|i| file(i, format!("f{i}").as_bytes(), 1))
            .collect();
        store.insert_entries(&job, entries).await.unwrap();
        let first = store.entries_page(&job, None, 10).await.unwrap();
        let second = store
            .entries_page(&job, Some(first.last().unwrap().entry.idx), 10)
            .await
            .unwrap();
        assert_eq!(first.len(), 10);
        assert_eq!(second[0].entry.idx, 10);
        let last = store.entries_page(&job, Some(19), 10).await.unwrap();
        assert_eq!(last.len(), 5);

        store.clear_manifest(&job).await.unwrap();
        assert!(store.entries_page(&job, None, 10).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn hundred_thousand_entries_in_batches() {
        let (store, job) = store_with_job().await;
        let start = std::time::Instant::now();
        for batch in 0..10u64 {
            let entries = (0..10_000u64)
                .map(|i| {
                    let idx = batch * 10_000 + i;
                    file(idx, format!("dir{batch}/file{i}.bin").as_bytes(), 1024)
                })
                .collect();
            store.insert_entries(&job, entries).await.unwrap();
        }
        let c = store.manifest_counts(&job).await.unwrap();
        assert_eq!(
            (c.files, c.pending, c.bytes),
            (100_000, 100_000, 100_000 * 1024)
        );
        eprintln!("inserted 100k entries in {:?}", start.elapsed());
    }
}
