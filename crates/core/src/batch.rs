//! One batch of the transfer Stage: a page of Pending Entries sent with one
//! [`Transport::transfer`] call, from however many Sources, with every Entry's transfer state
//! persisted as the transport reports it.
//!
//! Entries go `Partial` before the call. A file the transport reports arrived (`ItemDone`) is
//! saved `Done` on the next 500 ms tick, so a crash or a pause loses at most that tick's files,
//! which are sent again. When the call answers, every Entry gets its outcome: `Done`, `Failed`,
//! or back to `Partial` for a retryable error or a full destination.
//!
//! A full destination is the transport's `Full` error, or (OpenSSH's sftp-server says only
//! "Failure") a fatal error on a file bigger than the free space measured right after: either
//! way the files go back to `Partial`, not `Failed`, and the batch ends [`Sent::Full`] so the
//! Job pauses until someone frees space (docs/design/specs/D03).
//!
//! A source that can't be read any more (a disk unplugged) pauses the Job the same way, with
//! its files back to `Partial`: checked before a batch starts, and for the Sources of the files
//! that failed (docs/design/specs/D02).

use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::path::PathBuf;
use std::time::Duration;

use filebridge_store::{EntryState, ManifestEntry, Store};
use filebridge_transport_api::{
    Destination, ErrorKind, InFlight, ItemOutcome, ProgressEvent, ProgressSink, RemotePath,
    TransferItem, TransferJob, TransferOptions, Transport, TransportError,
};
use tokio::sync::broadcast;

use crate::analyse::{bytes_path, source_parent};
use crate::engine::PREFLIGHT_TIMEOUT;
use crate::sources::source_unavailable;
use crate::{CoreError, Event, Result};
use filebridge_model::{DoneView, FileCategory, InFlightView};

/// How often finished files are saved and announced.
const TICK: Duration = Duration::from_millis(500);

/// Why a batch stopped before the transport answered.
pub(crate) enum Stop {
    Cancelled,
    /// The send window closed.
    Held,
}

/// How a batch ended.
pub(crate) enum Sent {
    /// The transport answered for every item; each Entry has its state. `retry` is the first
    /// retryable error of an item, whose Entry went back to `Partial`.
    Answered { retry: Option<String> },
    /// The whole call failed with a retryable error; its Entries stay `Partial`.
    Retry(String),
    /// The destination is full: the files it refused for that went back to `Partial`, the
    /// others have their outcome.
    Full(String),
    /// A Source can't be read (a disk unplugged): its files are `Pending` (checked before the
    /// call) or back to `Partial`, the others have their outcome.
    SourceGone(String),
    /// Stopped by `stop`; files still in flight stay `Partial`.
    Stopped(Stop),
}

/// What the transfer Stage has sent so far, across its batches: the base the transport's
/// per-call counts are added to, and the job log's summary of the run.
#[derive(Default)]
pub(crate) struct Tally {
    pub bytes_done: u64,
    pub files_done: u64,
    pub sent: u64,
    pub failed: u64,
    pub bytes: u64,
    pub first_error: Option<String>,
}

/// Everything one batch needs from the Engine.
pub(crate) struct Batch<'a> {
    pub store: &'a Store,
    pub events: &'a broadcast::Sender<Event>,
    pub transport: &'a dyn Transport,
    pub dest: &'a Destination,
    pub job_id: &'a str,
    /// The Job's Sources, by `ManifestEntry::entry.source`.
    pub sources: &'a [PathBuf],
    pub concurrency: usize,
    /// The whole Stage's files and bytes, for progress.
    pub files_total: u64,
    pub bytes_total: u64,
}

impl Batch<'_> {
    /// Sends `entries`, or stops when `stop` resolves first. `tally` grows by what arrived.
    pub(crate) async fn send(
        &self,
        entries: Vec<ManifestEntry>,
        tally: &mut Tally,
        stop: impl Future<Output = Stop>,
    ) -> Result<Sent> {
        let mut roots: Vec<PathBuf> = Vec::new();
        let mut root_of: HashMap<PathBuf, usize> = HashMap::new();
        let mut items = Vec::with_capacity(entries.len());
        let mut kept = Vec::with_capacity(entries.len());
        let mut updates = Vec::new();
        for e in entries {
            let dest = e
                .dest_path
                .clone()
                .ok_or_else(|| "no destination name".to_owned())
                .and_then(|p| RemotePath::new(p).map_err(|err| err.message));
            let dest = match dest {
                Ok(dest) => dest,
                Err(msg) => {
                    updates.push((e.entry.idx, EntryState::Failed, Some(msg)));
                    continue;
                }
            };
            let parent = self
                .sources
                .get(e.entry.source as usize)
                .map(|s| source_parent(s))
                .ok_or_else(|| CoreError::Invalid(format!("unknown source {}", e.entry.source)))?;
            let root = *root_of.entry(parent.clone()).or_insert_with(|| {
                roots.push(parent);
                roots.len() - 1
            });
            items.push(TransferItem {
                root,
                source: bytes_path(&e.entry.raw_path),
                dest,
                size: e.entry.size,
                expected_hash: None,
            });
            kept.push(e);
        }
        self.store
            .set_transfer_states(self.job_id, std::mem::take(&mut updates))
            .await?;
        if items.is_empty() {
            return Ok(Sent::Answered { retry: None });
        }
        let used: Vec<u32> = kept.iter().map(|e| e.entry.source).collect();
        if let Some(why) = self.gone_sources(&used).await.into_values().next() {
            return Ok(Sent::SourceGone(why));
        }
        let job = TransferJob {
            id: format!("{}-{}", self.job_id, kept[0].entry.idx),
            source_roots: roots,
            items,
            options: TransferOptions {
                concurrency: self.concurrency,
            },
        };
        self.store
            .set_transfer_states(
                self.job_id,
                kept.iter()
                    .map(|e| (e.entry.idx, EntryState::Partial, None))
                    .collect(),
            )
            .await?;

        let (sink, rx) = ProgressSink::channel();
        let forward = tokio::spawn(forward(
            rx,
            Forwarder {
                store: self.store.clone(),
                events: self.events.clone(),
                job_id: self.job_id.to_owned(),
                idx_of: kept.iter().map(|e| e.entry.idx).collect(),
                queue: QueueNames::of(&kept),
                base: (tally.bytes_done, tally.files_done),
                totals: (self.bytes_total, self.files_total),
            },
        ));
        let answered = tokio::select! {
            r = self.transport.transfer(self.dest, &job, sink) => Ok(r),
            s = stop => Err(s),
        };
        // The transfer dropped its sink either way: the forwarder saves what it was told
        // arrived, then ends.
        let wait = if answered.is_ok() { 1 } else { 2 };
        let saved = tokio::time::timeout(Duration::from_secs(wait), forward)
            .await
            .ok()
            .and_then(|r| r.ok())
            .unwrap_or_default();
        let result = match answered {
            Ok(r) => r,
            Err(stop) => return Ok(Sent::Stopped(stop)),
        };
        let result = match result {
            Ok(r) => r,
            Err(e) if e.is_retryable() => return Ok(Sent::Retry(e.message)),
            Err(e) if e.kind == ErrorKind::Full => return Ok(Sent::Full(e.message)),
            Err(e) => {
                return match self.gone_sources(&used).await.into_values().next() {
                    Some(why) => Ok(Sent::SourceGone(why)),
                    None => Err(e.into()),
                };
            }
        };
        let mut items = result.items;
        let failed_from: Vec<u32> = kept
            .iter()
            .zip(&items)
            .filter(|(_, o)| matches!(o, ItemOutcome::Failed(err) if err.kind != ErrorKind::Full))
            .map(|(e, _)| e.entry.source)
            .collect();
        let gone = self.gone_sources(&failed_from).await;
        self.explain_failures(&kept, &mut items, &gone).await;

        let mut retry = None;
        let mut full = None;
        let mut source_gone = None;
        let mut any_done = false;
        for (i, (e, outcome)) in kept.iter().zip(items).enumerate() {
            match outcome {
                ItemOutcome::Done => {
                    any_done = true;
                    tally.bytes_done += e.entry.size;
                    tally.files_done += 1;
                    tally.sent += 1;
                    tally.bytes += e.entry.size;
                    if !saved.contains(&i) {
                        updates.push((e.entry.idx, EntryState::Done, None));
                    }
                }
                ItemOutcome::Failed(err) if gone.contains_key(&e.entry.source) => {
                    // Its disk is gone, not the file: it goes again once the disk is back.
                    source_gone.get_or_insert_with(|| gone[&e.entry.source].clone());
                    updates.push((e.entry.idx, EntryState::Partial, Some(err.message)));
                }
                ItemOutcome::Failed(err) if err.kind == ErrorKind::Full => {
                    // Not this file's fault: it goes again once there is room.
                    full.get_or_insert_with(|| err.message.clone());
                    updates.push((e.entry.idx, EntryState::Partial, Some(err.message)));
                }
                ItemOutcome::Failed(err) => {
                    let path = String::from_utf8_lossy(&e.entry.raw_path).into_owned();
                    let _ = self.events.send(Event::EntryFailed {
                        job_id: self.job_id.to_owned(),
                        path,
                        error: err.message.clone(),
                        retryable: err.is_retryable(),
                    });
                    tally.failed += 1;
                    tally.first_error.get_or_insert_with(|| err.message.clone());
                    let state = if err.is_retryable() {
                        retry.get_or_insert_with(|| err.message.clone());
                        EntryState::Partial
                    } else {
                        EntryState::Failed
                    };
                    updates.push((e.entry.idx, state, Some(err.message)));
                }
            }
        }
        self.store.set_transfer_states(self.job_id, updates).await?;
        if any_done {
            self.store.set_job_retry(self.job_id, 0, None).await?;
        }
        Ok(match (full, source_gone) {
            (Some(e), _) => Sent::Full(e),
            (None, Some(e)) => Sent::SourceGone(e),
            (None, None) => Sent::Answered { retry },
        })
    }

    /// The Sources among `used` (indexes, repeats allowed) that can't be read now, with why.
    async fn gone_sources(&self, used: &[u32]) -> HashMap<u32, String> {
        let mut distinct: Vec<u32> = used.to_vec();
        distinct.sort_unstable();
        distinct.dedup();
        let paths: Vec<(u32, PathBuf)> = distinct
            .into_iter()
            .filter_map(|i| Some((i, self.sources.get(i as usize)?.clone())))
            .collect();
        if paths.is_empty() {
            return HashMap::new();
        }
        tokio::task::spawn_blocking(move || {
            paths
                .into_iter()
                .filter_map(|(i, p)| Some((i, source_unavailable(&p)?)))
                .collect()
        })
        .await
        .unwrap_or_default()
    }

    /// Turns the fatal failures of files bigger than the destination's free space, measured now,
    /// into `Full`: a server that reports a full disk without saying so (OpenSSH's sftp-server
    /// answers "Failure"). Nothing changes when no file failed that way, or the destination
    /// doesn't report its space.
    /// Files of a Source in `gone` are left as they are: their disk is the reason.
    async fn explain_failures(
        &self,
        kept: &[ManifestEntry],
        items: &mut [ItemOutcome],
        gone: &HashMap<u32, String>,
    ) {
        let unexplained = |(e, o): (&ManifestEntry, &ItemOutcome)| {
            matches!(o, ItemOutcome::Failed(err) if err.kind == ErrorKind::Other)
                && !gone.contains_key(&e.entry.source)
        };
        if !kept.iter().zip(items.iter()).any(unexplained) {
            return;
        }
        let measured = tokio::time::timeout(PREFLIGHT_TIMEOUT, self.transport.preflight(self.dest));
        let Ok(Ok(report)) = measured.await else {
            return;
        };
        let Some(free) = report.free_bytes else {
            return;
        };
        for (e, outcome) in kept.iter().zip(items.iter_mut()) {
            if let ItemOutcome::Failed(err) = outcome
                && err.kind == ErrorKind::Other
                && !gone.contains_key(&e.entry.source)
                && e.entry.size > free
            {
                *err = TransportError::full(format!(
                    "no space left at the destination ({free} bytes free, the file is {}): {}",
                    e.entry.size, err.message
                ));
            }
        }
    }
}

/// What the progress forwarder of one batch holds.
struct Forwarder {
    store: Store,
    events: broadcast::Sender<Event>,
    job_id: String,
    /// Manifest idx by item index.
    idx_of: Vec<u64>,
    queue: QueueNames,
    /// What earlier batches sent, (bytes, files).
    base: (u64, u64),
    /// The Stage's (bytes, files).
    totals: (u64, u64),
}

impl Forwarder {
    /// Saves the items in `finished` `Done`, and adds them to `saved`.
    async fn save(&self, finished: &mut Vec<usize>, saved: &mut HashSet<usize>) {
        let indexes = std::mem::take(finished);
        let updates = indexes
            .iter()
            .filter_map(|&i| self.idx_of.get(i).map(|&idx| (idx, EntryState::Done, None)))
            .collect();
        match self.store.set_transfer_states(&self.job_id, updates).await {
            Ok(()) => saved.extend(indexes),
            Err(e) => tracing::warn!(job = self.job_id, "cannot save progress: {e}"),
        }
    }
}

/// Forwards the transport's progress as events, and saves the files it reports arrived on
/// each tick. Returns the item indexes it saved `Done`.
async fn forward(
    mut rx: tokio::sync::mpsc::UnboundedReceiver<ProgressEvent>,
    f: Forwarder,
) -> HashSet<usize> {
    let mut saved = HashSet::new();
    let mut finished: Vec<usize> = Vec::new();
    let mut arrived = Vec::new();
    let mut tick = tokio::time::interval(TICK);
    loop {
        tokio::select! {
            ev = rx.recv() => match ev {
                Some(ProgressEvent::Stats(s)) => {
                    let (bb, bf) = f.base;
                    let (bytes_total, files_total) = f.totals;
                    let bytes_done = bb + s.bytes;
                    let eta_secs = (s.bytes_per_sec > 0.0).then(|| {
                        (bytes_total.saturating_sub(bytes_done) as f64 / s.bytes_per_sec) as u64
                    });
                    let _ = f.events.send(Event::TransferProgress {
                        job_id: f.job_id.clone(),
                        bytes_done,
                        bytes_total,
                        files_done: bf + s.files_done,
                        files_total,
                        bytes_per_sec: s.bytes_per_sec,
                        eta_secs,
                        in_flight: s.transferring.iter().map(|x| f.queue.in_flight(x)).collect(),
                    });
                }
                Some(ProgressEvent::ItemDone { index, ok: true }) => {
                    finished.push(index);
                    if let Some(done) = f.queue.done(index) {
                        arrived.push(done);
                    }
                }
                Some(_) => {}
                None => break,
            },
            _ = tick.tick() => {
                if !finished.is_empty() {
                    f.save(&mut finished, &mut saved).await;
                }
                if !arrived.is_empty() {
                    let _ = f.events.send(entries_done(&f.job_id, std::mem::take(&mut arrived)));
                }
            }
        }
    }
    if !finished.is_empty() {
        f.save(&mut finished, &mut saved).await;
    }
    if !arrived.is_empty() {
        let _ = f.events.send(entries_done(&f.job_id, arrived));
    }
    saved
}

/// Most files listed in one `EntriesDone`; the rest are counted.
const ENTRIES_DONE_LISTED: usize = 20;

fn entries_done(job_id: &str, mut arrived: Vec<DoneView>) -> Event {
    let more = arrived.len().saturating_sub(ENTRIES_DONE_LISTED) as u64;
    arrived.truncate(ENTRIES_DONE_LISTED);
    Event::EntriesDone {
        job_id: job_id.to_owned(),
        entries: arrived,
        more,
    }
}

/// One batch's items as the transfer queue shows them (docs/design/specs/D04): by item index
/// for `ItemDone`, by destination path for the transport's files in flight.
struct QueueNames {
    items: Vec<QueueItem>,
    by_dest: HashMap<String, usize>,
}

struct QueueItem {
    idx: u64,
    dest: String,
    category: FileCategory,
    /// The source name, when it differs from the destination name.
    renamed_from: Option<String>,
}

impl QueueNames {
    fn of(kept: &[ManifestEntry]) -> Self {
        let items: Vec<QueueItem> = kept
            .iter()
            .map(|e| {
                let dest = e.dest_path.clone().unwrap_or_default();
                let source_name = file_name(&String::from_utf8_lossy(&e.entry.raw_path)).to_owned();
                QueueItem {
                    idx: e.entry.idx,
                    category: FileCategory::of_name(&e.entry.raw_path),
                    renamed_from: (source_name != file_name(&dest)).then_some(source_name),
                    dest,
                }
            })
            .collect();
        let by_dest = items
            .iter()
            .enumerate()
            .map(|(i, item)| (item.dest.clone(), i))
            .collect();
        Self { items, by_dest }
    }

    fn in_flight(&self, f: &InFlight) -> InFlightView {
        let item = self.by_dest.get(&f.dest).map(|&i| &self.items[i]);
        let (dir, name) = match f.dest.rsplit_once('/') {
            Some((dir, name)) => (dir.to_owned(), name.to_owned()),
            None => (String::new(), f.dest.clone()),
        };
        InFlightView {
            idx: item.map(|i| i.idx),
            category: item.map_or_else(|| FileCategory::of_name(name.as_bytes()), |i| i.category),
            renamed_from: item.and_then(|i| i.renamed_from.clone()),
            name,
            dir,
            size: f.size,
            bytes: f.bytes,
            bytes_per_sec: f.bytes_per_sec,
            eta_secs: f.eta_secs,
        }
    }

    fn done(&self, index: usize) -> Option<DoneView> {
        self.items.get(index).map(|i| DoneView {
            idx: i.idx,
            name: file_name(&i.dest).to_owned(),
            category: i.category,
        })
    }
}

fn file_name(path: &str) -> &str {
    path.rsplit_once('/').map_or(path, |(_, n)| n)
}
