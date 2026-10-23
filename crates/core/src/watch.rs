//! Continuous sync (docs/adr/0023): a watched schedule (`Recurrence::OnChange`) runs when its
//! sources change. The host gives the engine a [`FolderWatcher`]; its callback only notes which
//! schedule changed, and the scheduler folds those notes into the store (`Engine::tick_schedules`),
//! so a burst of events is one write and, a quiet time later, one run.
//!
//! Where the watcher can't watch (a watch limit, a folder that is gone), the schedule is scanned
//! instead: its safety rescan comes sooner. A run is a full scan either way, so a watch that
//! starts (the service starting, the schedule saved) makes the schedule due at once.

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use filebridge_model::{WatchMode, WatchView};
use filebridge_store::ScheduleRow;
use tokio::sync::Notify;

use crate::analyse::bytes_path;
use crate::{Engine, Event, Recurrence, Result};

/// The rescan of a watched schedule while the system reports its changes: events can be lost.
pub const RESCAN_WATCHING: Duration = Duration::from_secs(60 * 60);
/// The rescan of a watched schedule the system can't watch: the only way to see its changes.
pub const RESCAN_SCANNING: Duration = Duration::from_secs(10 * 60);
/// The longest changes wait for their sources to go quiet: a folder that never stops changing
/// still sends this often (or every quiet time, when that is longer).
pub const MAX_BATCH_WAIT: Duration = Duration::from_secs(15 * 60);
/// How soon a watched schedule due while its last run is unfinished looks again (it also looks
/// when any job finishes).
pub(crate) const BUSY_RECHECK: Duration = Duration::from_secs(60);

/// Watches folders for the engine (implemented by the host with the `notify` crate; by a fake in
/// tests).
pub trait FolderWatcher: Send + Sync {
    /// Starts watching `roots` (files or folders, folders recursively). `on_signal` may be called
    /// from any thread; watching stops when the returned handle is dropped.
    fn watch(
        &self,
        roots: &[PathBuf],
        on_signal: SignalFn,
    ) -> std::result::Result<WatchHandle, WatchError>;
}

/// What a watch reports.
pub type SignalFn = Arc<dyn Fn(WatchSignal) + Send + Sync>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WatchSignal {
    /// Something under the roots was created, written, renamed or lost track of (a queue
    /// overflow): the schedule should run. Removals need no signal (nothing is ever deleted at
    /// the destination).
    Changed,
    /// The watch stopped working (a root removed, a new folder past the watch limit).
    Broken(WatchError),
}

/// Why a folder can't be watched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WatchError {
    pub message: String,
    /// The system's limit on watched folders (Linux `fs.inotify.max_user_watches`): retried only
    /// when the schedule is saved or the service restarts, since trying walks the whole tree.
    pub limit: bool,
}

impl fmt::Display for WatchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

/// Keeps a watch alive; dropping it stops the watch.
pub struct WatchHandle(#[allow(dead_code)] Box<dyn Send + Sync>);

impl WatchHandle {
    pub fn new(inner: impl Send + Sync + 'static) -> Self {
        Self(Box::new(inner))
    }
}

/// The engine's watches: one per enabled watched schedule, while a scheduler runs.
#[derive(Default)]
pub(crate) struct Watches {
    watcher: Mutex<Option<Arc<dyn FolderWatcher>>>,
    active: Mutex<HashMap<String, Active>>,
    pub(crate) book: Arc<ChangeBook>,
}

struct Active {
    /// `None` while scanning instead.
    handle: Option<WatchHandle>,
    error: Option<WatchError>,
}

/// What the watches' callbacks noted since the scheduler last looked.
#[derive(Default)]
pub(crate) struct ChangeBook {
    notes: Mutex<Notes>,
    /// Wakes the scheduler.
    pub(crate) wake: Notify,
}

#[derive(Default)]
struct Notes {
    changed: HashSet<String>,
    broken: HashMap<String, WatchError>,
}

impl ChangeBook {
    fn note(&self, schedule_id: &str, signal: WatchSignal) {
        {
            let mut notes = self.notes.lock().unwrap();
            match signal {
                WatchSignal::Changed => {
                    // Already noted: the scheduler wakes once for the whole burst.
                    if !notes.changed.insert(schedule_id.to_owned()) {
                        return;
                    }
                }
                WatchSignal::Broken(e) => {
                    notes.broken.insert(schedule_id.to_owned(), e);
                }
            }
        }
        self.wake.notify_one();
    }

    fn take(&self) -> Notes {
        std::mem::take(&mut *self.notes.lock().unwrap())
    }
}

impl Watches {
    fn watcher(&self) -> Option<Arc<dyn FolderWatcher>> {
        self.watcher.lock().unwrap().clone()
    }

    /// Stops a schedule's watch, so the next look starts it again (and rescans).
    pub(crate) fn forget(&self, schedule_id: &str) {
        self.active.lock().unwrap().remove(schedule_id);
    }

    fn forget_all(&self) {
        self.active.lock().unwrap().clear();
    }

    /// A watched run couldn't read its sources (a disk unplugged): a watch on them is likely
    /// stale, so it is scanned instead and tried again at its next rescan.
    pub(crate) fn scan_instead(&self, schedule_id: &str, message: String) {
        if let Some(a) = self.active.lock().unwrap().get_mut(schedule_id) {
            a.handle = None;
            a.error = Some(WatchError {
                message,
                limit: false,
            });
        }
    }

    /// Whether the schedule is watched by events (not scanned instead, nor unwatched).
    fn by_events(&self, schedule_id: &str) -> bool {
        self.active
            .lock()
            .unwrap()
            .get(schedule_id)
            .is_some_and(|a| a.handle.is_some())
    }

    pub(crate) fn view(&self, row: &ScheduleRow, recurrence: &Recurrence) -> Option<WatchView> {
        recurrence.quiet_secs()?;
        let active = self.active.lock().unwrap();
        let (mode, error) = match active.get(&row.id) {
            Some(Active {
                handle: Some(_), ..
            }) => (WatchMode::Events, None),
            Some(Active { error, .. }) => (WatchMode::Scanning, error.clone()),
            None => (WatchMode::Idle, None),
        };
        Some(WatchView {
            mode,
            limit: error.as_ref().is_some_and(|e| e.limit),
            reason: error.map(|e| e.message),
            last_change_at: row.last_change_at,
            pending_since: row.pending_since,
        })
    }
}

/// When a watched schedule runs after changes noted at `now`: a quiet time after the last one,
/// but no later than [`MAX_BATCH_WAIT`] after the first one not sent yet.
pub(crate) fn after_change(now: i64, pending_since: Option<i64>, quiet_secs: u32) -> i64 {
    let quiet = i64::from(quiet_secs) * 1000;
    let cap = (MAX_BATCH_WAIT.as_millis() as i64).max(quiet);
    (now + quiet).min(pending_since.unwrap_or(now) + cap)
}

fn ms(d: Duration) -> i64 {
    d.as_millis() as i64
}

impl Engine {
    /// Gives the engine the watcher its watched schedules use (the service's; tests a fake).
    /// Without one, watched schedules are only scanned, every [`RESCAN_SCANNING`].
    pub fn set_watcher(&self, watcher: Arc<dyn FolderWatcher>) {
        *self.watches.watcher.lock().unwrap() = Some(watcher);
        self.watches.forget_all();
        self.schedules_changed.notify_one();
    }

    /// The next safety rescan of a watched schedule that has just run.
    pub(crate) fn rescan_after(&self, schedule_id: &str, now: i64) -> i64 {
        now + ms(if self.watches.by_events(schedule_id) {
            RESCAN_WATCHING
        } else {
            RESCAN_SCANNING
        })
    }

    /// Starts and stops watches to match the enabled watched schedules in `rows`, folds the
    /// changes noted since the last look into the store, and switches a broken watch to
    /// scanning. A watch that starts makes its schedule due at `now`: what changed while nobody
    /// watched is caught up by that run.
    pub(crate) async fn sync_watches(&self, rows: &[ScheduleRow], now: i64) -> Result<()> {
        let watched: HashMap<&str, (&ScheduleRow, u32)> = rows
            .iter()
            .filter(|r| r.enabled)
            .filter_map(|r| {
                let recurrence: Recurrence = serde_json::from_value(r.recurrence.clone()).ok()?;
                Some((r.id.as_str(), (r, recurrence.quiet_secs()?)))
            })
            .collect();
        let notes = self.watches.book.take();
        let mut changed_mode = Vec::new();

        if let Some(watcher) = self.watches.watcher() {
            let mut start = Vec::new();
            {
                let mut active = self.watches.active.lock().unwrap();
                active.retain(|id, _| watched.contains_key(id.as_str()));
                for (id, error) in &notes.broken {
                    if let Some(a) = active.get_mut(id) {
                        tracing::warn!(
                            schedule = id,
                            "watching stopped, scanning instead: {error}"
                        );
                        a.handle = None;
                        a.error = Some(error.clone());
                        changed_mode.push(id.clone());
                    }
                }
                for (id, (row, _)) in &watched {
                    match active.get(*id) {
                        None => start.push((*row, true)),
                        // Scanning for a reason other than the limit: try again when its
                        // rescan is due.
                        Some(Active {
                            handle: None,
                            error,
                        }) if !error.as_ref().is_some_and(|e| e.limit)
                            && row.next_run_at.is_some_and(|t| t <= now) =>
                        {
                            start.push((*row, false))
                        }
                        Some(_) => {}
                    }
                }
            }
            for (row, new) in start {
                let roots: Vec<PathBuf> = row.sources.iter().map(|s| bytes_path(s)).collect();
                let book = self.watches.book.clone();
                let id = row.id.clone();
                let on_signal: SignalFn = Arc::new(move |s| book.note(&id, s));
                let active = match watcher.watch(&roots, on_signal) {
                    Ok(handle) => Active {
                        handle: Some(handle),
                        error: None,
                    },
                    Err(error) => {
                        if new {
                            tracing::warn!(
                                schedule = row.id,
                                "can't watch, scanning instead: {error}"
                            );
                        }
                        Active {
                            handle: None,
                            error: Some(error),
                        }
                    }
                };
                let by_events = active.handle.is_some();
                self.watches
                    .active
                    .lock()
                    .unwrap()
                    .insert(row.id.clone(), active);
                if new || by_events {
                    changed_mode.push(row.id.clone());
                }
                if new {
                    self.store.set_next_run(&row.id, Some(now)).await?;
                }
            }
        }

        for id in &changed_mode {
            if notes.broken.contains_key(id) {
                // Events may have been lost before it broke.
                self.store.set_next_run(id, Some(now)).await?;
            }
        }
        for id in &notes.changed {
            let Some((row, quiet)) = watched.get(id.as_str()) else {
                continue;
            };
            let next = after_change(now, row.pending_since, *quiet);
            self.store.note_schedule_change(id, now, Some(next)).await?;
        }
        let refreshed: HashSet<&String> = changed_mode.iter().chain(&notes.changed).collect();
        for id in refreshed {
            self.emit(Event::ScheduleChanged {
                schedule_id: id.clone(),
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_burst_waits_for_quiet_but_not_forever() {
        // The first change: a quiet time later.
        assert_eq!(after_change(1_000, None, 60), 61_000);
        // Changes keep coming: each pushes it on, until 15 minutes after the first.
        assert_eq!(after_change(500_000, Some(1_000), 60), 560_000);
        assert_eq!(after_change(899_000, Some(1_000), 60), 901_000);
        assert_eq!(after_change(950_000, Some(1_000), 60), 901_000);
        // A quiet time longer than the cap is the cap.
        assert_eq!(after_change(1_000, Some(1_000), 3600), 3_601_000);
    }

    #[test]
    fn a_burst_wakes_the_scheduler_once() {
        let book = ChangeBook::default();
        for _ in 0..1000 {
            book.note("s1", WatchSignal::Changed);
        }
        book.note(
            "s2",
            WatchSignal::Broken(WatchError {
                message: "gone".into(),
                limit: false,
            }),
        );
        let notes = book.take();
        assert_eq!(notes.changed.len(), 1);
        assert_eq!(notes.broken.len(), 1);
        assert!(book.take().changed.is_empty());
    }
}
