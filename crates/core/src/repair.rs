//! "Repair destination" (docs/04): names already at a destination go through the same rules and
//! decisions as a send, then are renamed **there**, and the integration is told about the folders
//! that changed. A job of its own (`JobKind::Repair`): stages 2–3 list the destination folder
//! instead of local sources, stage 4 renames instead of sending, and stage 6 is shared.
//!
//! The folder being repaired is kept as the job's `target_dir`, so the name plan's paths land
//! under it exactly as a send's land under its target folder. Nothing is ever deleted or
//! overwritten: a rename whose new name is taken fails on its own.

use std::collections::{BTreeSet, VecDeque};

use filebridge_names::{Entry, NamePlan};
use filebridge_store::{EntryState, ManifestEntry, NewJob};
use filebridge_transport_api::{ErrorKind, RemotePath, TransportCaps};

use crate::analyse::{self, Analysis};
use crate::browse::under;
use crate::{CoreError, Engine, Event, JobKind, JobState, Profile, Result, SendOptions};

/// The most entries one repair job looks at; pick a smaller folder beyond that.
pub const MAX_REPAIR_ENTRIES: usize = 500_000;

const PAGE: u32 = 2_000;

/// `dir` as a folder under the destination root, `None` for the root itself.
fn folder(dir: Option<String>) -> Result<Option<String>> {
    let dir = dir
        .map(|d| d.trim_matches('/').to_owned())
        .filter(|d| !d.is_empty());
    if let Some(d) = &dir {
        RemotePath::new(d.clone())?;
    }
    Ok(dir)
}

fn remote(path: String) -> Result<Option<RemotePath>> {
    Ok(if path.is_empty() {
        None
    } else {
        Some(RemotePath::new(path)?)
    })
}

/// `rel` under the repaired folder; the folder itself for an empty `rel`.
fn at(dir: Option<&str>, rel: &str) -> String {
    if rel.is_empty() {
        dir.unwrap_or_default().to_owned()
    } else {
        under(dir, rel.to_owned())
    }
}

/// The last component of a `/`-separated path.
fn last(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

impl Engine {
    /// Creates a draft Repair job for `dir` (the destination root when `None`) of a profile whose
    /// transport can list and rename. Nothing is read from the destination yet.
    pub async fn create_repair_job(&self, profile_id: &str, dir: Option<String>) -> Result<String> {
        let profile = self.get_profile(profile_id).await?;
        self.registry.check_profile(&profile)?;
        let transport = self.registry.transport(&profile.destination.transport)?;
        if !transport
            .capabilities()
            .contains(TransportCaps::LIST | TransportCaps::MOVE)
        {
            return Err(CoreError::Invalid(
                "this destination can't list and rename its files".into(),
            ));
        }
        let dir = folder(dir)?;
        let options = SendOptions {
            verify: false,
            target_dir: dir.clone(),
            ..SendOptions::default()
        };
        let id = uuid::Uuid::new_v4().to_string();
        self.store
            .insert_job(NewJob {
                id: id.clone(),
                kind: JobKind::Repair.as_str().into(),
                profile_id: profile.id,
                state: JobState::Draft.as_str().into(),
                options: serde_json::to_value(&options)?,
                sources: Vec::new(),
                label: None,
                again_of: None,
            })
            .await?;
        self.log(
            Some(&id),
            "created",
            serde_json::json!({ "repair": dir.unwrap_or_default(), "profile_id": profile_id }),
        )
        .await?;
        Ok(id)
    }

    /// Stage 2 for Repair: the tree under the repaired folder, one folder at a time, parents
    /// before children, then stage 3 over it. Paths are relative to the folder.
    pub(crate) async fn scan_destination(
        &self,
        job_id: &str,
        profile: &Profile,
        dir: Option<&str>,
        names: &NamePlan,
    ) -> Result<Analysis> {
        let transport = self.registry.transport(&profile.destination.transport)?;
        let mut entries: Vec<Entry> = Vec::new();
        let mut mtimes = Vec::new();
        let (mut files, mut bytes) = (0u64, 0u64);
        let mut queue: VecDeque<(Option<usize>, String)> = VecDeque::from([(None, String::new())]);
        while let Some((parent, rel)) = queue.pop_front() {
            let here = remote(at(dir, &rel))?;
            for e in transport
                .list(&profile.destination, here.as_ref(), false)
                .await?
            {
                if entries.len() >= MAX_REPAIR_ENTRIES {
                    return Err(CoreError::Invalid(format!(
                        "the folder holds more than {MAX_REPAIR_ENTRIES} entries; pick a folder \
                         inside it"
                    )));
                }
                let path = if rel.is_empty() {
                    e.name.clone()
                } else {
                    format!("{rel}/{}", e.name)
                };
                let idx = entries.len();
                let size = e.size.unwrap_or(0);
                if e.is_dir {
                    queue.push_back((Some(idx), path.clone()));
                } else {
                    files += 1;
                    bytes += size;
                }
                entries.push(Entry::new(
                    path.into_bytes(),
                    e.name.into_bytes(),
                    size,
                    e.is_dir,
                    parent,
                ));
                mtimes.push(e.mtime);
            }
            self.emit(Event::ScanProgress {
                job_id: job_id.to_owned(),
                files,
                bytes,
                current: Some(at(dir, &rel)),
            });
        }
        let source_of = vec![0; entries.len()];
        Ok(analyse::build(
            &entries,
            &mtimes,
            &source_of,
            names,
            Vec::new(),
            0,
        ))
    }

    /// Stage 4 for Repair: renames every entry whose own name changes, deepest first, so each
    /// rename happens under its parent's old path. Saved per entry: a resumed job never renames
    /// twice, and an entry whose old name is gone and new name is there counts as done.
    pub(crate) async fn repair_stage(
        &self,
        job_id: &str,
        profile: &Profile,
        dir: Option<&str>,
        cancel: &tokio_util::sync::CancellationToken,
    ) -> Result<RepairEnd> {
        let transport = self.registry.transport(&profile.destination.transport)?;
        let dest = &profile.destination;
        let mut todo: Vec<ManifestEntry> = Vec::new();
        let mut after = None;
        loop {
            let page = self.store.entries_page(job_id, after, PAGE).await?;
            let Some(last_entry) = page.last() else { break };
            after = Some(last_entry.entry.idx);
            todo.extend(
                page.into_iter()
                    .filter(|e| e.transfer_state == EntryState::Pending),
            );
        }
        let depth = |e: &ManifestEntry| e.entry.raw_path.iter().filter(|b| **b == b'/').count();
        todo.sort_by_key(|e| std::cmp::Reverse(depth(e)));

        let counts = self.store.manifest_counts(job_id).await?;
        let total = counts.files + counts.dirs;
        let first = total - todo.len() as u64 + 1;
        for (done, e) in (first..).zip(todo) {
            if cancel.is_cancelled() {
                return Ok(RepairEnd::Cancelled);
            }
            let old = under(dir, String::from_utf8_lossy(&e.entry.raw_path).into_owned());
            let state = match &e.dest_path {
                None => (EntryState::Skipped, None),
                Some(new) if last(new) == last(&old) => (EntryState::Skipped, None),
                Some(new) => {
                    let target = match old.rsplit_once('/') {
                        Some((parent, _)) => format!("{parent}/{}", last(new)),
                        None => last(new).to_owned(),
                    };
                    let (from, to) = (RemotePath::new(old.clone())?, RemotePath::new(target)?);
                    match transport.move_path(dest, &from, &to).await {
                        Ok(()) => (EntryState::Done, None),
                        Err(err) if err.kind == ErrorKind::NotFound => {
                            // Renamed before a crash or a pause: done if the new name is there.
                            let parent = remote(to.parent().unwrap_or("").to_owned())?;
                            let there = transport
                                .list(dest, parent.as_ref(), false)
                                .await
                                .is_ok_and(|l| l.iter().any(|x| x.name == to.file_name()));
                            if there {
                                (EntryState::Done, None)
                            } else {
                                (EntryState::Failed, Some(err.message))
                            }
                        }
                        Err(err) if err.is_retryable() => return Ok(RepairEnd::Retry(err.message)),
                        Err(err) => (EntryState::Failed, Some(err.message)),
                    }
                }
            };
            self.store
                .set_transfer_states(job_id, vec![(e.entry.idx, state.0, state.1)])
                .await?;
            self.emit(Event::TransferProgress {
                job_id: job_id.to_owned(),
                bytes_done: 0,
                bytes_total: 0,
                files_done: done,
                files_total: total,
                bytes_per_sec: 0.0,
                eta_secs: None,
                in_flight: Vec::new(),
            });
        }
        Ok(RepairEnd::Done)
    }

    /// What stage 6 is told changed after a repair: each folder that holds a renamed entry,
    /// under its new name (the integration rescans it, dropping the old names and finding the
    /// new ones), or the renamed entries themselves when they sit at the destination root.
    /// Folders inside another listed folder are left out: rescanning the outer one covers them.
    pub(crate) async fn repaired_folders(
        &self,
        job_id: &str,
        dir: Option<&str>,
    ) -> Result<Vec<RemotePath>> {
        let mut out: BTreeSet<String> = BTreeSet::new();
        let mut after = None;
        loop {
            let page = self.store.entries_page(job_id, after, PAGE).await?;
            let Some(last_entry) = page.last() else { break };
            after = Some(last_entry.entry.idx);
            for e in page {
                if e.transfer_state != EntryState::Done {
                    continue;
                }
                let Some(new) = e.dest_path else { continue };
                let parent = new.rsplit_once('/').map(|(p, _)| p.to_owned());
                out.insert(parent.or_else(|| dir.map(str::to_owned)).unwrap_or(new));
            }
        }
        let all: Vec<String> = out.into_iter().collect();
        let kept = all
            .iter()
            .filter(|p| {
                !all.iter()
                    .any(|q| q != *p && p.starts_with(q.as_str()) && p[q.len()..].starts_with('/'))
            })
            .map(|p| RemotePath::new(p.clone()))
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(kept)
    }
}

/// How the rename stage ended.
pub(crate) enum RepairEnd {
    Done,
    Retry(String),
    Cancelled,
}
