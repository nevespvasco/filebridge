//! A job's files for the Files tab and the Entry detail (docs/design/specs/D05).

use filebridge_model::{
    EntryCursor, EntryDetail, EntryFacets, EntryFilter, EntryPage, EntrySort, EntryStatus,
    EntryView, FileCategory, HashView, StatusCount,
};
use filebridge_store::{EntryKey, EntryOrder, EntryQuery, EntryRow, ManifestEntry};

use crate::analyse::{bytes_path, source_parent};
use crate::browse::under;
use crate::{Engine, JobKind, Result, SendOptions};

/// The most rows one page may have.
pub const MAX_ENTRY_PAGE: u32 = 500;

impl Engine {
    /// One page of a job's files, filtered and ordered.
    pub async fn files_page(
        &self,
        job_id: &str,
        filter: EntryFilter,
        after: Option<EntryCursor>,
        limit: u32,
    ) -> Result<EntryPage> {
        self.store.get_job(job_id).await?;
        let limit = limit.clamp(1, MAX_ENTRY_PAGE);
        let query = EntryQuery {
            states: filter
                .statuses
                .iter()
                .map(|s| {
                    let (t, v) = s.states();
                    (t.to_owned(), v.map(str::to_owned))
                })
                .collect(),
            categories: filter
                .categories
                .iter()
                .map(|c| c.as_str().to_owned())
                .collect(),
            renamed: filter.renamed,
            include_dirs: filter.include_dirs,
            text: filter.query.map(|q| q.trim().to_owned()),
            order: match filter.sort {
                EntrySort::FolderOrder => EntryOrder::Scan,
                EntrySort::LargestFirst => EntryOrder::LargestFirst,
                EntrySort::ProblemsFirst => EntryOrder::ProblemsFirst,
            },
        };
        let after = after.map(|c| EntryKey {
            idx: c.idx,
            size: c.size,
            rank: c.rank,
        });
        let rows = self
            .store
            .entries_filtered(job_id, query, after, limit)
            .await?;
        let next = (rows.len() == limit as usize).then(|| {
            let k = rows.last().expect("a full page").key();
            EntryCursor {
                idx: k.idx,
                size: k.size,
                rank: k.rank,
            }
        });
        Ok(EntryPage {
            items: rows.iter().map(view).collect(),
            next,
        })
    }

    /// Counts for the filter chips, over the whole job.
    pub async fn entry_facets(&self, job_id: &str) -> Result<EntryFacets> {
        let (states, renamed) = self.store.entry_state_counts(job_id).await?;
        let mut by_status: Vec<StatusCount> = Vec::new();
        for (t, v, n) in states {
            let status = EntryStatus::from_states(&t, &v);
            match by_status.iter_mut().find(|s| s.status == status) {
                Some(s) => s.files += n,
                None => by_status.push(StatusCount { status, files: n }),
            }
        }
        by_status.sort_by_key(|s| EntryStatus::ALL.iter().position(|x| *x == s.status));
        let problems = by_status
            .iter()
            .filter(|s| s.status.is_problem())
            .map(|s| s.files)
            .sum();
        Ok(EntryFacets {
            by_category: self.job_categories(job_id).await?,
            by_status,
            renamed,
            problems,
        })
    }

    /// Everything about one entry.
    pub async fn entry_detail(&self, job_id: &str, idx: u64) -> Result<EntryDetail> {
        let job = self.store.get_job(job_id).await?;
        let profile = self.profile_for_history(&job.profile_id).await?;
        let e = self.store.get_entry(job_id, idx).await?;
        let issues = self.store.issues_for_entry(job_id, idx).await?;
        let renamed = issues
            .iter()
            .any(|i| matches!(i.issue.decision.as_str(), "accepted" | "edited"));
        let row = EntryRow {
            entry: e.clone(),
            renamed,
            rank: 1,
        };
        let root = profile.destination.root.trim_end_matches('/');
        let at_root = |d: &str| {
            if root.is_empty() {
                d.to_owned()
            } else {
                format!("{root}/{d}")
            }
        };
        let source_path = if JobKind::parse(&job.kind)? == JobKind::Repair {
            // A repair's entries come from the destination: where the name was found there.
            let options: SendOptions = serde_json::from_value(job.options).unwrap_or_default();
            let rel = String::from_utf8_lossy(&e.entry.raw_path).into_owned();
            at_root(&under(options.target_dir.as_deref(), rel))
        } else {
            let source_root = job
                .sources
                .get(e.entry.source as usize)
                .map(|s| source_parent(&bytes_path(s)))
                .unwrap_or_default();
            source_root
                .join(bytes_path(&e.entry.raw_path))
                .to_string_lossy()
                .into_owned()
        };
        let source_path_hex = std::str::from_utf8(&e.entry.raw_path)
            .is_err()
            .then(|| hex(&e.entry.raw_path));
        let path_of_raw = String::from_utf8_lossy(&e.entry.raw_path).into_owned();
        let mut views = Vec::with_capacity(issues.len());
        for r in issues {
            views.push(self.issue_view(r, path_of_raw.clone()));
        }
        Ok(EntryDetail {
            view: view(&row),
            source_path,
            source_path_hex,
            dest_path: e.dest_path.as_deref().map(at_root),
            mtime: e.entry.mtime,
            hash: e.entry.hash.as_ref().map(|h| HashView {
                algo: e.entry.hash_algo.clone().unwrap_or_else(|| "sha256".into()),
                hex: hex(h),
            }),
            issues: views,
            error: e.error.clone(),
        })
    }
}

fn view(r: &EntryRow) -> EntryView {
    let e: &ManifestEntry = &r.entry;
    let source = String::from_utf8_lossy(&e.entry.raw_path).into_owned();
    let path = e.dest_path.clone().unwrap_or_else(|| source.clone());
    let (dir, name) = match path.rsplit_once('/') {
        Some((d, n)) => (d.to_owned(), n.to_owned()),
        None => (String::new(), path.clone()),
    };
    let source_name = source.rsplit_once('/').map_or(source.as_str(), |(_, n)| n);
    EntryView {
        idx: e.entry.idx,
        is_dir: e.entry.is_dir,
        source_name: (r.renamed && source_name != name).then(|| source_name.to_owned()),
        name,
        dir,
        size: e.entry.size,
        category: FileCategory::parse(&e.entry.category).unwrap_or(FileCategory::Other),
        status: EntryStatus::from_states(e.transfer_state.as_str(), e.verify_state.as_str()),
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
