//! The engine's folder watcher (docs/adr/0023) on the `notify` crate: inotify on Linux, FSEvents
//! on macOS, `ReadDirectoryChangesW` on Windows. Only `filebridge-service` gives it to the engine.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use filebridge_core::{FolderWatcher, SignalFn, WatchError, WatchHandle, WatchSignal};
use notify::event::{EventKind, MetadataKind, ModifyKind};
use notify::{RecursiveMode, Watcher};

/// Watches with the system's file events.
#[derive(Debug, Default, Clone, Copy)]
pub struct NotifyWatcher;

impl FolderWatcher for NotifyWatcher {
    fn watch(&self, roots: &[PathBuf], on_signal: SignalFn) -> Result<WatchHandle, WatchError> {
        let mut watchers = Vec::with_capacity(roots.len());
        for root in roots {
            let meta = std::fs::metadata(root).map_err(|e| WatchError {
                message: format!("{}: {e}", root.display()),
                limit: false,
            })?;
            // A file is watched through its folder (editors save by replacing the file, which
            // would end a watch on the file itself), and only its own name counts there.
            let (path, mode, only) = if meta.is_dir() {
                (root.clone(), RecursiveMode::Recursive, None)
            } else {
                let parent = root.parent().unwrap_or(Path::new("/")).to_path_buf();
                (
                    parent,
                    RecursiveMode::NonRecursive,
                    root.file_name().map(|n| n.to_owned()),
                )
            };
            let on_signal = on_signal.clone();
            let root_seen = root.clone();
            let mut watcher =
                notify::recommended_watcher(move |res: notify::Result<notify::Event>| match res {
                    Ok(event) => {
                        if let Some(s) = signal_of(&event, &root_seen, only.as_deref()) {
                            on_signal(s);
                        }
                    }
                    Err(e) => on_signal(WatchSignal::Broken(watch_error(&e))),
                })
                .map_err(|e| watch_error(&e))?;
            watcher.watch(&path, mode).map_err(|e| watch_error(&e))?;
            watchers.push(watcher);
        }
        Ok(WatchHandle::new(watchers))
    }
}

/// What an event under `root` means for the schedule; `None` for events that need no send
/// (reads, removals, other files next to a watched file `only`).
fn signal_of(event: &notify::Event, root: &Path, only: Option<&OsStr>) -> Option<WatchSignal> {
    if event.need_rescan() {
        return Some(WatchSignal::Changed);
    }
    if let Some(name) = only
        && !event.paths.iter().any(|p| p.file_name() == Some(name))
    {
        return None;
    }
    match event.kind {
        EventKind::Access(_) => None,
        // Removing a watched folder itself ends its watch; any other removal needs no send (a
        // watched file is watched through its folder, which goes on).
        EventKind::Remove(_) if only.is_none() && event.paths.iter().any(|p| p == root) => {
            Some(WatchSignal::Broken(WatchError {
                message: format!("{} is gone", root.display()),
                limit: false,
            }))
        }
        EventKind::Remove(_) => None,
        EventKind::Modify(ModifyKind::Metadata(MetadataKind::AccessTime)) => None,
        EventKind::Create(_) | EventKind::Modify(_) | EventKind::Any | EventKind::Other => {
            Some(WatchSignal::Changed)
        }
    }
}

fn watch_error(e: &notify::Error) -> WatchError {
    let limit = match &e.kind {
        notify::ErrorKind::MaxFilesWatch => true,
        // inotify instances (`fs.inotify.max_user_instances`).
        notify::ErrorKind::Io(io) => io.raw_os_error() == Some(EMFILE),
        _ => false,
    };
    let message = if limit {
        "the system's limit on watched folders was reached".to_owned()
    } else {
        e.to_string()
    };
    WatchError { message, limit }
}

/// "Too many open files" (`EMFILE`, 24 on Linux and macOS).
const EMFILE: i32 = 24;

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    use notify::event::{CreateKind, RemoveKind};

    use super::*;

    fn event(kind: EventKind, path: &str) -> notify::Event {
        notify::Event::new(kind).add_path(PathBuf::from(path))
    }

    #[test]
    fn what_needs_a_send() {
        let dir = Path::new("/home/u/Fotos");
        let file = Path::new("/home/u/notes.txt");
        let name = file.file_name();
        let changed = Some(WatchSignal::Changed);
        let create = event(EventKind::Create(CreateKind::File), "/home/u/Fotos/a.jpg");
        assert_eq!(signal_of(&create, dir, None), changed);
        let edit = |p| event(EventKind::Modify(ModifyKind::Any), p);
        assert_eq!(signal_of(&edit("/home/u/notes.txt"), file, name), changed);
        // A neighbour of a watched file, a read, a removal: nothing.
        assert_eq!(signal_of(&edit("/home/u/other.txt"), file, name), None);
        let read = event(
            EventKind::Access(notify::event::AccessKind::Any),
            "/home/u/Fotos/a.jpg",
        );
        assert_eq!(signal_of(&read, dir, None), None);
        let removed = event(EventKind::Remove(RemoveKind::File), "/home/u/Fotos/a.jpg");
        assert_eq!(signal_of(&removed, dir, None), None);
        // The root itself removed: the watch is over.
        let gone = event(EventKind::Remove(RemoveKind::Folder), "/home/u/Fotos");
        assert!(matches!(
            signal_of(&gone, dir, None),
            Some(WatchSignal::Broken(WatchError { limit: false, .. }))
        ));
        // A watched file removed (an editor saving by delete and create): its folder's watch
        // goes on.
        let file_gone = event(EventKind::Remove(RemoveKind::File), "/home/u/notes.txt");
        assert_eq!(signal_of(&file_gone, file, name), None);
        // Lost events: a rescan.
        let overflow = notify::Event::new(EventKind::Other).set_flag(notify::event::Flag::Rescan);
        assert_eq!(signal_of(&overflow, dir, None), changed);
        assert!(watch_error(&notify::Error::new(notify::ErrorKind::MaxFilesWatch)).limit);
        assert!(!watch_error(&notify::Error::path_not_found()).limit);
    }

    #[test]
    fn a_real_folder_reports_a_new_file_and_a_missing_one_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("Acme");
        std::fs::create_dir_all(root.join("sub")).unwrap();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let sink = seen.clone();
        let handle = NotifyWatcher
            .watch(
                std::slice::from_ref(&root),
                Arc::new(move |s| sink.lock().unwrap().push(s)),
            )
            .unwrap();
        std::fs::write(root.join("sub/new.txt"), b"x").unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while !seen.lock().unwrap().contains(&WatchSignal::Changed) {
            assert!(Instant::now() < deadline, "no event for a new file");
            std::thread::sleep(Duration::from_millis(20));
        }
        drop(handle);

        let missing = NotifyWatcher.watch(&[dir.path().join("nope")], Arc::new(|_| {}));
        assert!(matches!(missing, Err(WatchError { limit: false, .. })));
    }
}
