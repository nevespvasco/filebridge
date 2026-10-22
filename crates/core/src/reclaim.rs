//! Whether a finished job's sources can be freed safely (docs/design/specs/D10, ADR 0011).
//! filebridge only **shows** it: nothing here, or anywhere, deletes, moves or changes a source.
//! The re-check reads the source tree and compares it with the manifest.

use std::collections::HashMap;

use filebridge_model::{
    BlockerView, RECLAIM_CHECK_FRESH_MS, ReclaimBlocker, ReclaimStatus, SourceReclaim,
};
use filebridge_store::now_ms;
use serde::{Deserialize, Serialize};

use crate::analyse::{bytes_path, os_bytes};
use crate::{Engine, Event, JobState, Result, SendOptions};

/// How many new or changed paths a re-check keeps, for "See them".
const SAMPLE: usize = 100;

/// What a re-check found, kept on the job.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct StoredCheck {
    at: i64,
    size_only: bool,
    sources: Vec<SourceCheck>,
    sample: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct SourceCheck {
    available: bool,
    changed: u64,
    new: u64,
    gone: u64,
}

#[derive(Default)]
struct SourceCounts {
    files: u64,
    bytes: u64,
    different: u64,
    unverifiable: u64,
    failed: u64,
    skipped: u64,
}

fn blocker(code: ReclaimBlocker, count: Option<u64>) -> BlockerView {
    BlockerView { code, count }
}

impl Engine {
    /// Whether a job's sources can be freed, and why not. Reads only what is stored.
    pub async fn reclaim_status(&self, job_id: &str) -> Result<ReclaimStatus> {
        let row = self.store.get_job(job_id).await?;
        let state = JobState::parse(&row.state)?;
        let options: SendOptions = serde_json::from_value(row.options.clone()).unwrap_or_default();
        let check: Option<StoredCheck> = row
            .reclaim_check
            .clone()
            .and_then(|v| serde_json::from_value(v).ok());

        let mut blockers = Vec::new();
        if state != JobState::Completed {
            blockers.push(blocker(ReclaimBlocker::NotCompleted, None));
        }
        if !options.verify {
            blockers.push(blocker(ReclaimBlocker::NotVerified, None));
        }
        let fresh_check = check
            .as_ref()
            .filter(|c| row.finished_at.is_some_and(|f| c.at >= f));
        match fresh_check {
            None => blockers.push(blocker(ReclaimBlocker::NotChecked, None)),
            Some(c) if now_ms() - c.at > RECLAIM_CHECK_FRESH_MS => {
                blockers.push(blocker(ReclaimBlocker::StaleCheck, None))
            }
            Some(_) => {}
        }

        let mut per: HashMap<u32, SourceCounts> = HashMap::new();
        for (source, transfer, verify, n, bytes) in self.store.source_state_counts(job_id).await? {
            let c = per.entry(source).or_default();
            c.files += n;
            c.bytes += bytes;
            match (transfer.as_str(), verify.as_str()) {
                ("done", "done") => {}
                ("done", "mismatch" | "missing") => c.different += n,
                ("done", _) => c.unverifiable += n,
                ("skipped", _) => c.skipped += n,
                _ => c.failed += n,
            }
        }

        let mut sources = Vec::with_capacity(row.sources.len());
        let mut bytes = 0;
        for (i, raw) in row.sources.iter().enumerate() {
            let c = per.remove(&(i as u32)).unwrap_or_default();
            let mut b = Vec::new();
            let count = |n: u64| (n > 0).then_some(n);
            if let Some(n) = count(c.different) {
                b.push(blocker(ReclaimBlocker::Different, Some(n)));
            }
            if let Some(n) = count(c.unverifiable) {
                b.push(blocker(ReclaimBlocker::Unverifiable, Some(n)));
            }
            if let Some(n) = count(c.failed) {
                b.push(blocker(ReclaimBlocker::Failed, Some(n)));
            }
            if let Some(n) = count(c.skipped) {
                b.push(blocker(ReclaimBlocker::Skipped, Some(n)));
            }
            if let Some(sc) = fresh_check.and_then(|ch| ch.sources.get(i)) {
                if !sc.available {
                    b.push(blocker(ReclaimBlocker::SourceUnavailable, None));
                } else if let Some(n) = count(sc.changed + sc.new) {
                    b.push(blocker(ReclaimBlocker::Changed, Some(n)));
                }
            }
            let eligible = b.is_empty() && blockers.is_empty();
            if eligible {
                bytes += c.bytes;
            }
            sources.push(SourceReclaim {
                path: String::from_utf8_lossy(raw).into_owned(),
                files: c.files,
                bytes: c.bytes,
                eligible,
                blockers: b,
            });
        }
        Ok(ReclaimStatus {
            job_id: job_id.to_owned(),
            eligible: blockers.is_empty() && sources.iter().all(|s| s.eligible),
            bytes,
            verified_at: row.finished_at,
            checked_at: check.as_ref().map(|c| c.at),
            size_only: check.as_ref().is_some_and(|c| c.size_only),
            changed_sample: fresh_check.map(|c| c.sample.clone()).unwrap_or_default(),
            sources,
            blockers,
            dismissed: row.reclaim_dismissed_at.is_some(),
        })
    }

    /// Walks the job's sources (read-only) and compares them with the manifest: files new or
    /// changed (size, or modification time when both are known) since the send. Keeps the
    /// result on the job, then answers the status.
    pub async fn recheck_sources(&self, job_id: &str) -> Result<ReclaimStatus> {
        let row = self.store.get_job(job_id).await?;
        let mut known: HashMap<(u32, Vec<u8>), (u64, Option<i64>)> = self
            .store
            .file_fingerprints(job_id)
            .await?
            .into_iter()
            .map(|(s, p, size, mtime)| ((s, p), (size, mtime)))
            .collect();
        let total = known.len() as u64;
        let size_only = known.values().any(|(_, m)| m.is_none());
        let mut check = StoredCheck {
            at: now_ms(),
            size_only,
            ..Default::default()
        };
        let mut checked = 0u64;
        for (i, raw) in row.sources.iter().enumerate() {
            let source = i as u32;
            let path = bytes_path(raw);
            let found = tokio::task::spawn_blocking(move || read_source(&path))
                .await
                .map_err(|e| crate::CoreError::Invalid(format!("source check failed: {e}")))?;
            let mut sc = SourceCheck::default();
            if let Some(files) = found {
                sc.available = true;
                for (rel, size, mtime) in files {
                    checked += 1;
                    match known.remove(&(source, rel.clone())) {
                        None => {
                            sc.new += 1;
                            sample(&mut check.sample, &rel);
                        }
                        Some((s, m)) => {
                            let differs =
                                s != size || matches!((m, mtime), (Some(a), Some(b)) if a != b);
                            if differs {
                                sc.changed += 1;
                                sample(&mut check.sample, &rel);
                            }
                        }
                    }
                }
                sc.gone = known.keys().filter(|(s, _)| *s == source).count() as u64;
            }
            check.sources.push(sc);
            self.emit(Event::ReclaimCheckProgress {
                job_id: job_id.to_owned(),
                checked,
                total,
            });
        }
        self.store
            .set_reclaim_check(job_id, serde_json::to_value(&check)?)
            .await?;
        self.reclaim_status(job_id).await
    }

    /// Hides a job from the reclaimable total, or shows it again. Records nothing about the
    /// source: filebridge never finds out whether it was freed.
    pub async fn dismiss_reclaim(&self, job_id: &str, dismissed: bool) -> Result<()> {
        self.store
            .set_reclaim_dismissed(job_id, dismissed.then(now_ms))
            .await?;
        Ok(())
    }

    /// Jobs whose sources can be freed safely, and not hidden.
    pub(crate) async fn reclaimable_ids(&self) -> Result<Vec<String>> {
        let mut out = Vec::new();
        for id in self.store.completed_job_ids().await? {
            let s = self.reclaim_status(&id).await?;
            if s.eligible && !s.dismissed {
                out.push(id);
            }
        }
        Ok(out)
    }
}

fn sample(list: &mut Vec<String>, raw: &[u8]) {
    if list.len() < SAMPLE {
        list.push(String::from_utf8_lossy(raw).into_owned());
    }
}

/// A file at the source: its manifest path, size and modification time.
type SourceFile = (Vec<u8>, u64, Option<i64>);

/// The files of one source root as the manifest names them (`<root name>/<relative path>`, or
/// the root's name for a single file), with size and modification time. `None` when the root
/// isn't there.
fn read_source(path: &std::path::Path) -> Option<Vec<SourceFile>> {
    let meta = std::fs::symlink_metadata(path).ok()?;
    let name = os_bytes(path.file_name()?);
    if !meta.is_dir() {
        return Some(vec![(
            name,
            meta.len(),
            filebridge_scanner::mtime_ms(&meta),
        )]);
    }
    let scan = filebridge_scanner::scan(path).ok()?;
    Some(
        scan.entries
            .into_iter()
            .zip(scan.mtimes)
            .filter(|(e, _)| !e.is_dir)
            .map(|(e, mtime)| {
                let mut raw = name.clone();
                raw.push(b'/');
                raw.extend_from_slice(&e.raw_path);
                (raw, e.size, mtime)
            })
            .collect(),
    )
}
