//! A chosen folder's size split by what is directly inside it (docs/design/specs/D02), measured
//! on request, streamed as `Event::FolderSizesProgress` while it runs, and cancellable. Read-only
//! like every look at a source: metadata and listings only.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use filebridge_model::{FOLDER_SIZES_LISTED, FolderSize, FolderSizes};
use filebridge_scanner::SizeTally;

use crate::{CoreError, Engine, Event, Result};

/// The most measurements that run at once.
pub const MAX_FOLDER_SIZE_SCANS: usize = 4;
/// How often a running measurement says how far it got.
const REPORT_EVERY: Duration = Duration::from_millis(250);

/// The measurements running now, by the id their caller gave, with the flag that stops each.
#[derive(Default)]
pub(crate) struct Scans(Mutex<HashMap<String, Arc<AtomicBool>>>);

/// Takes a measurement's id off the list once it ends, and stops it if its caller went away
/// (the future was dropped) before it did.
struct Running<'a> {
    scans: &'a Scans,
    id: String,
    stop: Arc<AtomicBool>,
}

impl Drop for Running<'_> {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        self.scans.0.lock().expect("scans lock").remove(&self.id);
    }
}

impl Engine {
    /// Measures the folder `path`: how many files and bytes each item directly inside it holds.
    /// `scan_id` names the measurement, for [`Engine::cancel_folder_sizes`] and the progress
    /// events. Cancelled, it answers with what it counted (`complete: false`).
    pub async fn folder_sizes(&self, scan_id: String, path: String) -> Result<FolderSizes> {
        let root = PathBuf::from(&path);
        if !root.is_absolute() || root.file_name().is_none() {
            return Err(CoreError::Invalid(format!(
                "not a folder that can be measured: {path}"
            )));
        }
        let stop = Arc::new(AtomicBool::new(false));
        {
            let mut scans = self.sizes.0.lock().expect("scans lock");
            if scans.contains_key(&scan_id) {
                return Err(CoreError::Invalid(format!(
                    "measurement {scan_id} is already running"
                )));
            }
            if scans.len() >= MAX_FOLDER_SIZE_SCANS {
                return Err(CoreError::Invalid(format!(
                    "at most {MAX_FOLDER_SIZE_SCANS} folders can be measured at once"
                )));
            }
            scans.insert(scan_id.clone(), stop.clone());
        }
        let _running = Running {
            scans: &self.sizes,
            id: scan_id.clone(),
            stop: stop.clone(),
        };
        let events = self.events.clone();
        tokio::task::spawn_blocking(move || {
            // The first report goes at once: a big folder shows numbers quickly.
            let mut last: Option<Instant> = None;
            let mut report = |t: &SizeTally| {
                if last.is_none_or(|l| l.elapsed() >= REPORT_EVERY) {
                    last = Some(Instant::now());
                    let _ = events.send(Event::FolderSizesProgress {
                        scan_id: scan_id.clone(),
                        sizes: view(&path, &root, t, false),
                    });
                }
            };
            let stopped = || stop.load(Ordering::Relaxed);
            filebridge_scanner::folder_sizes(&root, &stopped, &mut report)
                .map(|(t, complete)| view(&path, &root, &t, complete))
                .map_err(|e| CoreError::Invalid(format!("can't measure {path}: {e}")))
        })
        .await
        .map_err(|e| CoreError::Invalid(format!("measurement failed: {e}")))?
    }

    /// Stops the measurement `scan_id`; it answers with what it counted. Nothing happens if it
    /// already ended.
    pub fn cancel_folder_sizes(&self, scan_id: &str) {
        if let Some(stop) = self.sizes.0.lock().expect("scans lock").get(scan_id) {
            stop.store(true, Ordering::Relaxed);
        }
    }
}

/// The tally as the UI shows it: the biggest items first, the rest summed.
fn view(path: &str, root: &Path, t: &SizeTally, complete: bool) -> FolderSizes {
    let mut order: Vec<usize> = (0..t.children.len()).collect();
    order.sort_by(|&a, &b| {
        let (a, b) = (&t.children[a], &t.children[b]);
        b.bytes.cmp(&a.bytes).then_with(|| a.name.cmp(&b.name))
    });
    let mut out = FolderSizes {
        path: path.to_owned(),
        items: Vec::new(),
        rest_items: 0,
        rest_files: 0,
        rest_bytes: 0,
        files: t.files,
        dirs: t.dirs,
        bytes: t.bytes,
        errors: t.errors,
        complete,
    };
    for (n, i) in order.into_iter().enumerate() {
        let c = &t.children[i];
        if n < FOLDER_SIZES_LISTED {
            out.items.push(FolderSize {
                name: c.name.to_string_lossy().into_owned(),
                path: root.join(&c.name).to_str().map(str::to_owned),
                is_dir: c.is_dir,
                files: c.files,
                bytes: c.bytes,
            });
        } else {
            out.rest_items += 1;
            out.rest_files += c.files;
            out.rest_bytes += c.bytes;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use filebridge_scanner::ChildSize;

    #[test]
    fn the_biggest_are_listed_and_the_rest_summed() {
        let children = (0..FOLDER_SIZES_LISTED as u64 + 2)
            .map(|i| ChildSize {
                name: format!("d{i:03}").into(),
                is_dir: true,
                files: 1,
                bytes: i,
            })
            .collect();
        let t = SizeTally {
            children,
            files: 202,
            dirs: 202,
            bytes: 1,
            errors: 0,
        };
        let v = view("/x", Path::new("/x"), &t, true);
        assert_eq!(v.items.len(), FOLDER_SIZES_LISTED);
        assert_eq!(v.items[0].name, "d201");
        assert_eq!(v.items[0].path.as_deref(), Some("/x/d201"));
        assert_eq!((v.rest_items, v.rest_files, v.rest_bytes), (2, 2, 1));
    }
}
