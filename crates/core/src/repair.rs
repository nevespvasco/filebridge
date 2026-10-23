//! "Repair destination" (docs/04): names already at a destination go through the same rules and
//! decisions as a send, then are renamed **there**, and the integration is told about the folders
//! that changed. A job of its own (`JobKind::Repair`): stages 2–3 list the destination folder
//! instead of local sources, stage 4 renames instead of sending, and stage 6 is shared.
//!
//! The folder being repaired is kept as the job's `target_dir`, so the name plan's paths land
//! under it exactly as a send's land under its target folder. Nothing is ever deleted or
//! overwritten: a rename whose new name is taken fails on its own (`Transport::move_path`).

use std::collections::{BTreeSet, HashMap, VecDeque};

use filebridge_names::{Entry, NamePlan, Planned, loose_key};
use filebridge_store::{EntryState, ManifestEntry, NewJob};
use filebridge_transport_api::{ErrorKind, MoveRequest, RemotePath, TransportCaps};

use crate::analyse::{self, Analysis};
use crate::browse::under;
use crate::decisions;
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
                schedule_id: None,
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
    /// before children, then stage 3 over it. Paths are relative to the folder. A subfolder
    /// that can't be read, or a name the destination gives back unreadable (not UTF-8: rclone
    /// shows such bytes as U+FFFD, and that name can't be asked for again), is a scan error and
    /// stays as it is; a failure to reach the destination at all stops the analysis.
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
        let mut scan_errors = Vec::new();
        let (mut files, mut bytes) = (0u64, 0u64);
        let mut queue: VecDeque<(Option<usize>, String)> = VecDeque::from([(None, String::new())]);
        while let Some((parent, rel)) = queue.pop_front() {
            let here = remote(at(dir, &rel))?;
            let listed = match transport
                .list(&profile.destination, here.as_ref(), false)
                .await
            {
                Ok(l) => l,
                Err(e)
                    if parent.is_some()
                        && !matches!(e.kind, ErrorKind::Transient | ErrorKind::Auth) =>
                {
                    scan_errors.push(format!("{}: {e}", at(dir, &rel)));
                    continue;
                }
                Err(e) => return Err(e.into()),
            };
            for e in listed {
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
                if e.name.contains('\u{fffd}') {
                    scan_errors.push(format!(
                        "{}: the name is not valid UTF-8 at the destination and can't be \
                         renamed there; rename it by hand",
                        at(dir, &path)
                    ));
                    continue;
                }
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
            scan_errors,
            0,
        ))
    }

    /// Stage 4 for Repair: renames every entry whose own name changes, deepest first, so each
    /// rename happens under its parent's old path; in one folder, an entry whose new name is
    /// another's old name waits for that one. Saved per entry: a resumed job never renames twice.
    /// Each rename goes through an intermediate name derived from the job and the entry, so
    /// one cut short is finished when the job resumes. Returns the warnings for the entries left
    /// under that intermediate name.
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
        let (mut files_total, mut files_done) = (0u64, 0u64);
        let mut after = None;
        loop {
            let page = self.store.entries_page(job_id, after, PAGE).await?;
            let Some(last_entry) = page.last() else { break };
            after = Some(last_entry.entry.idx);
            for e in page {
                if !e.entry.is_dir {
                    files_total += 1;
                    files_done += u64::from(e.transfer_state != EntryState::Pending);
                }
                if e.transfer_state == EntryState::Pending {
                    todo.push(e);
                }
            }
        }
        let todo = rename_order(todo, dir);

        let mut warnings = Vec::new();
        for e in todo {
            if cancel.is_cancelled() {
                return Ok(RepairEnd::Cancelled);
            }
            let old = under(dir, String::from_utf8_lossy(&e.entry.raw_path).into_owned());
            let state = match new_name(&e, &old) {
                None => (EntryState::Skipped, None),
                Some(name) => {
                    let parent = old.rsplit_once('/').map(|(p, _)| p);
                    let sibling = |n: &str| RemotePath::new(join(parent, n));
                    let req = MoveRequest {
                        from: RemotePath::new(old.clone())?,
                        to: sibling(name)?,
                        via: sibling(&intermediate(job_id, e.entry.idx))?,
                        is_dir: e.entry.is_dir,
                        size: (!e.entry.is_dir).then_some(e.entry.size),
                        mtime: e.entry.mtime,
                    };
                    match transport.move_path(dest, &req).await {
                        Ok(()) => (EntryState::Done, None),
                        Err(err) if err.is_retryable() => return Ok(RepairEnd::Retry(err.message)),
                        Err(err) => {
                            // Report an entry left under the intermediate name: one listing,
                            // on failures only.
                            let folder = remote(parent.unwrap_or_default().to_owned())?;
                            let left = transport
                                .list(dest, folder.as_ref(), false)
                                .await
                                .is_ok_and(|l| l.iter().any(|x| x.name == req.via.file_name()));
                            if left {
                                warnings.push(format!(
                                    "{old} is left as {}; rename it back by hand",
                                    req.via
                                ));
                            }
                            (EntryState::Failed, Some(err.message))
                        }
                    }
                }
            };
            self.store
                .set_transfer_states(job_id, vec![(e.entry.idx, state.0, state.1)])
                .await?;
            if e.entry.is_dir {
                continue;
            }
            files_done += 1;
            self.emit(Event::TransferProgress {
                job_id: job_id.to_owned(),
                bytes_done: 0,
                bytes_total: 0,
                files_done,
                files_total,
                bytes_per_sec: 0.0,
                eta_secs: None,
                in_flight: Vec::new(),
            });
        }
        Ok(RepairEnd::Done(warnings))
    }

    /// How many entries a Repair job would rename with its decisions as they are: what its
    /// confirmation counts. Entries that are skipped, or whose name ends up as it is, don't
    /// count. Pending decisions are refused, as at Send.
    pub async fn planned_renames(&self, job_id: &str) -> Result<u64> {
        let names = self.job_name_plan(job_id).await?;
        let entries = self.entries(job_id).await?;
        let decided: Vec<_> = self
            .all_issues(job_id, None)
            .await?
            .into_iter()
            .map(decisions::decided)
            .collect();
        let planned = names.resolve(&entries, &decided)?;
        let renamed = entries.iter().zip(planned).filter(|(e, p)| match p {
            Planned::Path(path) => last(path).as_bytes() != e.raw_name.as_slice(),
            Planned::Skipped => false,
        });
        Ok(renamed.count() as u64)
    }

    /// What a finished repair changed: each folder that holds a renamed entry, where it is now
    /// (under its new name if its own rename worked, its old one if not), for stage 6 to
    /// rescan; or the renamed entries themselves when they sit at the destination root.
    /// Folders inside another listed folder are left out: rescanning the outer one covers
    /// them. Also how many entries (files and folders) failed to be renamed.
    pub(crate) async fn repair_outcome(
        &self,
        job_id: &str,
        dir: Option<&str>,
    ) -> Result<RepairOutcome> {
        let mut out: BTreeSet<String> = BTreeSet::new();
        let mut failed = 0;
        // Where each entry is now, by index (entries come in scanner order, parents first).
        let mut now: HashMap<u64, String> = HashMap::new();
        let mut after = None;
        loop {
            let page = self.store.entries_page(job_id, after, PAGE).await?;
            let Some(last_entry) = page.last() else { break };
            after = Some(last_entry.entry.idx);
            for e in page {
                let raw = String::from_utf8_lossy(&e.entry.raw_path).into_owned();
                let old_name = last(&raw).to_owned();
                let renamed = e.transfer_state == EntryState::Done;
                failed += u64::from(e.transfer_state == EntryState::Failed);
                let name = match (&e.dest_path, renamed) {
                    (Some(new), true) => last(new).to_owned(),
                    _ => old_name,
                };
                let parent = match e.entry.parent_idx {
                    Some(p) => now.get(&p).cloned(),
                    None => dir.map(str::to_owned),
                };
                let path = join(parent.as_deref(), &name);
                if renamed {
                    out.insert(parent.unwrap_or_else(|| path.clone()));
                }
                if e.entry.is_dir {
                    now.insert(e.entry.idx, path);
                }
            }
        }
        let all: Vec<String> = out.into_iter().collect();
        let changed = all
            .iter()
            .filter(|p| {
                !all.iter()
                    .any(|q| q != *p && p.starts_with(q.as_str()) && p[q.len()..].starts_with('/'))
            })
            .map(|p| RemotePath::new(p.clone()))
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(RepairOutcome { changed, failed })
    }
}

/// What a finished repair changed ([`Engine::repair_outcome`]).
pub(crate) struct RepairOutcome {
    pub changed: Vec<RemotePath>,
    pub failed: u64,
}

/// How the rename stage ended.
pub(crate) enum RepairEnd {
    /// With warnings about entries left under an intermediate name.
    Done(Vec<String>),
    Retry(String),
    Cancelled,
}

fn join(parent: Option<&str>, name: &str) -> String {
    parent.map_or(name.to_owned(), |p| format!("{p}/{name}"))
}

/// The entry's new own name, when it changes.
fn new_name<'a>(e: &'a ManifestEntry, old: &str) -> Option<&'a str> {
    let new = last(e.dest_path.as_deref()?);
    (new != last(old)).then_some(new)
}

/// The intermediate name of one entry's rename: the same every time the job asks, so a rename
/// cut short between its hops is found and finished.
fn intermediate(job_id: &str, idx: u64) -> String {
    let job: String = job_id
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .take(12)
        .collect();
    format!(".fb-repair-{job}-{idx}")
}

/// The order renames run in: deepest first, so each happens under its parent's old path; and
/// in one folder, an entry whose new name is another's old name after that other, so the name
/// is free by then (a cycle, `a`↔`b`, keeps its order: one of them fails as taken).
fn rename_order(mut todo: Vec<ManifestEntry>, dir: Option<&str>) -> Vec<ManifestEntry> {
    let depth = |e: &ManifestEntry| e.entry.raw_path.iter().filter(|b| **b == b'/').count();
    todo.sort_by_key(|e| (std::cmp::Reverse(depth(e)), e.entry.parent_idx, e.entry.idx));
    let mut out: Vec<ManifestEntry> = Vec::with_capacity(todo.len());
    let mut rest = todo.into_iter().peekable();
    while let Some(first) = rest.next() {
        let key = (depth(&first), first.entry.parent_idx);
        let mut group = vec![first];
        while let Some(e) = rest.next_if(|e| (depth(e), e.entry.parent_idx) == key) {
            group.push(e);
        }
        out.extend(free_names_first(group, dir));
    }
    out
}

/// One folder's renames, each after the entry whose old name it takes.
fn free_names_first(group: Vec<ManifestEntry>, dir: Option<&str>) -> Vec<ManifestEntry> {
    let old_of =
        |e: &ManifestEntry| under(dir, String::from_utf8_lossy(&e.entry.raw_path).into_owned());
    let by_old: HashMap<String, usize> = group
        .iter()
        .enumerate()
        .map(|(i, e)| (loose_key(last(&old_of(e))), i))
        .collect();
    let takes: Vec<Option<usize>> = group
        .iter()
        .enumerate()
        .map(|(i, e)| {
            let new = new_name(e, &old_of(e))?;
            by_old.get(&loose_key(new)).copied().filter(|j| *j != i)
        })
        .collect();
    // Follow each chain of "takes the old name of" and place it from its far end.
    let mut placed = vec![false; group.len()];
    let mut order = Vec::with_capacity(group.len());
    for start in 0..group.len() {
        let mut chain = Vec::new();
        let mut i = start;
        while !placed[i] && !chain.contains(&i) {
            chain.push(i);
            match takes[i] {
                Some(j) => i = j,
                None => break,
            }
        }
        for i in chain.into_iter().rev() {
            placed[i] = true;
            order.push(i);
        }
    }
    let mut slots: Vec<Option<ManifestEntry>> = group.into_iter().map(Some).collect();
    order.into_iter().filter_map(|i| slots[i].take()).collect()
}
