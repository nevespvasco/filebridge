//! Stages 2 and 3 for a set of sources: scan (names as bytes), run the name rules, and find
//! destination collisions. Synchronous; the engine runs it on a blocking thread.
//!
//! Each source root is itself an entry, so sending `/home/u/Acme` creates `Acme/…` at the
//! destination and the root's own name goes through the rules too. Paths in the manifest are
//! relative to the source's parent directory.

use std::collections::HashMap;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use filebridge_model::FileCategory;
use filebridge_names::{Entry, NamePlan, Severity};
use filebridge_store::{Decision, NewEntry, NewIssue};

use crate::{CoreError, Result};

pub(crate) struct Analysis {
    pub entries: Vec<NewEntry>,
    pub dest_paths: Vec<(u64, Option<String>)>,
    pub issues: Vec<NewIssue>,
    pub files: u64,
    pub dirs: u64,
    pub bytes: u64,
    pub scan_errors: Vec<String>,
    pub skipped_symlinks: u64,
    /// Files left out because a schedule sent them before and they haven't changed.
    pub unchanged: u64,
    /// Files left out because they changed within the watched schedule's quiet time.
    pub unsettled: u64,
}

/// What a schedule sent before (docs/adr/0019), by source index and manifest path: each file's
/// size and mtime then.
pub(crate) type SentBefore = HashMap<(u32, Vec<u8>), (u64, Option<i64>)>;

/// A watched run's "still being written" test (docs/adr/0023): a file whose mtime is after
/// `now - quiet_ms` (and not after `now`, a clock it can't judge) waits for the next run.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Settle {
    pub now: i64,
    pub quiet_ms: i64,
}

impl Settle {
    fn unsettled(&self, mtime: Option<i64>) -> bool {
        mtime.is_some_and(|m| m > self.now - self.quiet_ms && m <= self.now)
    }
}

/// Raw bytes of an OS string (`/`-separated on Unix).
pub(crate) fn os_bytes(s: &OsStr) -> Vec<u8> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        s.as_bytes().to_vec()
    }
    #[cfg(not(unix))]
    {
        s.to_string_lossy().replace('\\', "/").into_bytes()
    }
}

/// Inverse of [`os_bytes`].
pub(crate) fn bytes_path(b: &[u8]) -> PathBuf {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        PathBuf::from(OsStr::from_bytes(b))
    }
    #[cfg(not(unix))]
    {
        PathBuf::from(String::from_utf8_lossy(b).into_owned())
    }
}

/// The directory a source's manifest paths are relative to.
pub(crate) fn source_parent(source: &Path) -> PathBuf {
    source.parent().map(Path::to_path_buf).unwrap_or_default()
}

/// Scans `sources` and runs the name rules. With `sent`, files a schedule sent before and that
/// still have the same size and mtime are left out, and so are folders with nothing left in them;
/// with `settle` too, so are files still being written.
pub(crate) fn analyse_sources(
    sources: &[PathBuf],
    names: &NamePlan,
    sent: Option<&SentBefore>,
    settle: Option<Settle>,
    mut on_source: impl FnMut(&Path, u64, u64),
) -> Result<Analysis> {
    let mut entries: Vec<Entry> = Vec::new();
    let mut mtimes: Vec<Option<i64>> = Vec::new();
    let mut source_of: Vec<u32> = Vec::new();
    let mut scan_errors = Vec::new();
    let mut skipped_symlinks = 0u64;

    for (si, source) in sources.iter().enumerate() {
        let name = source.file_name().ok_or_else(|| {
            CoreError::Invalid(format!("{} cannot be sent as a source", source.display()))
        })?;
        let name = os_bytes(name);
        let meta = std::fs::symlink_metadata(source)?;
        if meta.file_type().is_symlink() {
            return Err(CoreError::Invalid(format!(
                "{} is a symbolic link; pick its target instead",
                source.display()
            )));
        }
        let root_id = entries.len();
        let size = if meta.is_dir() { 0 } else { meta.len() };
        entries.push(Entry::new(
            name.clone(),
            name.clone(),
            size,
            meta.is_dir(),
            None,
        ));
        mtimes.push(filebridge_scanner::mtime_ms(&meta));
        source_of.push(si as u32);
        let (mut files, mut bytes) = (u64::from(!meta.is_dir()), size);
        if meta.is_dir() {
            let scan = filebridge_scanner::scan(source)?;
            mtimes.extend(scan.mtimes.iter().copied());
            for e in scan.entries {
                let mut raw_path = name.clone();
                raw_path.push(b'/');
                raw_path.extend_from_slice(&e.raw_path);
                let parent = Some(e.parent.map_or(root_id, |p| root_id + 1 + p));
                if !e.is_dir {
                    files += 1;
                    bytes += e.size;
                }
                entries.push(Entry::new(raw_path, e.raw_name, e.size, e.is_dir, parent));
                source_of.push(si as u32);
            }
            skipped_symlinks += scan.skipped_symlinks.len() as u64;
            scan_errors.extend(
                scan.errors
                    .into_iter()
                    .map(|(p, e)| format!("{}: {e}", p.display())),
            );
        }
        on_source(source, files, bytes);
    }

    let (mut unchanged, mut unsettled) = (0, 0);
    if let Some(sent) = sent {
        (entries, mtimes, source_of, unchanged, unsettled) =
            drop_unchanged(entries, mtimes, source_of, sent, settle);
    }
    let mut analysis = build(
        &entries,
        &mtimes,
        &source_of,
        names,
        scan_errors,
        skipped_symlinks,
    );
    analysis.unchanged = unchanged;
    analysis.unsettled = unsettled;
    Ok(analysis)
}

/// Leaves out the files `sent` has with the same size and mtime, those `settle` finds still
/// being written, and the folders with nothing left under them, keeping scanner order and
/// renumbering the parents. Returns how many files were left out as unchanged and as unsettled.
fn drop_unchanged(
    entries: Vec<Entry>,
    mtimes: Vec<Option<i64>>,
    source_of: Vec<u32>,
    sent: &SentBefore,
    settle: Option<Settle>,
) -> (Vec<Entry>, Vec<Option<i64>>, Vec<u32>, u64, u64) {
    let changed: Vec<bool> = entries
        .iter()
        .enumerate()
        .map(|(i, e)| {
            !e.is_dir
                && sent.get(&(source_of[i], e.raw_path.clone()))
                    != Some(&(e.size, mtimes.get(i).copied().flatten()))
        })
        .collect();
    let unchanged = entries
        .iter()
        .zip(&changed)
        .filter(|(e, c)| !e.is_dir && !**c)
        .count() as u64;
    let mut unsettled = 0;
    let mut keep = changed;
    if let Some(settle) = settle {
        for (i, k) in keep.iter_mut().enumerate() {
            if *k && settle.unsettled(mtimes.get(i).copied().flatten()) {
                *k = false;
                unsettled += 1;
            }
        }
    }
    // Parents come before their children: walking back, a kept entry keeps its parent.
    for i in (0..entries.len()).rev() {
        if keep[i]
            && let Some(p) = entries[i].parent
        {
            keep[p] = true;
        }
    }
    let mut new_idx = vec![usize::MAX; entries.len()];
    let (mut out, mut out_mtimes, mut out_source) = (Vec::new(), Vec::new(), Vec::new());
    for (i, mut e) in entries.into_iter().enumerate() {
        if !keep[i] {
            continue;
        }
        new_idx[i] = out.len();
        e.parent = e.parent.map(|p| new_idx[p]);
        out.push(e);
        out_mtimes.push(mtimes.get(i).copied().flatten());
        out_source.push(source_of[i]);
    }
    (out, out_mtimes, out_source, unchanged, unsettled)
}

/// Stage 3 over scanned entries (local sources, or a destination folder for Repair): the name
/// rules' proposals, the issues, and the manifest rows. `entries` in scanner order (parents
/// first); `mtimes` and `source_of` by entry.
pub(crate) fn build(
    entries: &[Entry],
    mtimes: &[Option<i64>],
    source_of: &[u32],
    names: &NamePlan,
    scan_errors: Vec<String>,
    skipped_symlinks: u64,
) -> Analysis {
    let mapped = names.propose(entries);

    let mut issues = Vec::new();
    let (mut files, mut dirs, mut bytes) = (0, 0, 0);
    let mut out_entries = Vec::with_capacity(entries.len());
    let mut dest_paths = Vec::with_capacity(entries.len());

    for (i, (entry, m)) in entries.iter().zip(mapped).enumerate() {
        let idx = i as u64;
        if entry.is_dir {
            dirs += 1;
        } else {
            files += 1;
            bytes += entry.size;
        }
        for issue in m.issues {
            let auto = issue.severity == Severity::Auto;
            issues.push(NewIssue {
                entry_idx: idx,
                rule: issue.rule.to_owned(),
                severity: if auto { "auto" } else { "needs_decision" }.into(),
                message_key: issue.message_key.to_owned(),
                before: issue.before,
                after: issue.after,
                decision: if auto {
                    Decision::Accepted
                } else {
                    Decision::Pending
                },
            });
        }
        out_entries.push(NewEntry {
            idx,
            source: source_of[i],
            raw_path: entry.raw_path.clone(),
            parent_idx: entry.parent.map(|p| p as u64),
            is_dir: entry.is_dir,
            size: entry.size,
            mtime: mtimes.get(i).copied().flatten(),
            hash: None,
            hash_algo: None,
            category: if entry.is_dir {
                FileCategory::Other
            } else {
                FileCategory::of_name(&entry.raw_path)
            }
            .as_str()
            .into(),
        });
        dest_paths.push((idx, m.dest_path));
    }

    Analysis {
        entries: out_entries,
        dest_paths,
        issues,
        files,
        dirs,
        bytes,
        scan_errors,
        skipped_symlinks,
        unchanged: 0,
        unsettled: 0,
    }
}

// macOS file systems (APFS, HFS+) treat the NFC and NFD spellings as one name, so the collision
// these tests need can't exist there; the other test needs Linux's undecodable names.
#[cfg(all(test, not(target_os = "macos")))]
mod tests {
    use super::*;
    use filebridge_names::{Nfc, RULE_COLLISION, RuleSet, TargetProfile};

    fn nfc_plan() -> NamePlan {
        NamePlan::new(
            RuleSet::new(vec![Box::new(Nfc)], &TargetProfile::default()),
            true,
        )
    }

    #[test]
    fn source_root_is_an_entry_and_nfd_collides_with_nfc() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("Acme");
        std::fs::create_dir_all(src.join("sub")).unwrap();
        std::fs::write(src.join("sub/a.txt"), b"aaa").unwrap();
        // "Verão.txt" twice: NFC and NFD. After NFC both map to the same name.
        std::fs::write(src.join("Ver\u{e3}o.txt"), b"1").unwrap();
        std::fs::write(src.join("Vera\u{303}o.txt"), b"22").unwrap();

        let mut seen = Vec::new();
        let a = analyse_sources(
            std::slice::from_ref(&src),
            &nfc_plan(),
            None,
            None,
            |p, f, b| seen.push((p.to_path_buf(), f, b)),
        )
        .unwrap();
        assert_eq!(seen, vec![(src, 3, 6)]);
        assert_eq!((a.files, a.dirs, a.bytes), (3, 2, 6));
        assert_eq!(a.entries[0].raw_path, b"Acme");
        assert_eq!(a.entries[0].parent_idx, None);
        assert!(a.entries[1..].iter().all(|e| e.parent_idx.is_some()));
        let paths: Vec<_> = a.dest_paths.iter().filter_map(|(_, p)| p.clone()).collect();
        assert!(paths.contains(&"Acme/sub/a.txt".to_owned()));
        // Proposed paths assume the proposals accepted: the second spelling takes the suffix.
        assert_eq!(paths.iter().filter(|p| *p == "Acme/Verão.txt").count(), 1);
        assert!(paths.contains(&"Acme/Verão (2).txt".to_owned()));

        assert_eq!(a.issues.iter().filter(|i| i.rule == "nfc").count(), 1);
        let collision: Vec<_> = a
            .issues
            .iter()
            .filter(|i| i.rule == RULE_COLLISION)
            .collect();
        assert_eq!(collision.len(), 1);
        assert_eq!(collision[0].after, "Verão (2).txt".as_bytes());
        assert_eq!(collision[0].decision, Decision::Pending);
    }

    #[test]
    fn files_sent_before_and_unchanged_are_left_out() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("Acme");
        std::fs::create_dir_all(src.join("old/deeper")).unwrap();
        std::fs::create_dir_all(src.join("mixed")).unwrap();
        std::fs::write(src.join("old/deeper/a.txt"), b"aaa").unwrap();
        std::fs::write(src.join("mixed/same.txt"), b"s").unwrap();
        std::fs::write(src.join("mixed/changed.txt"), b"now longer").unwrap();
        std::fs::write(src.join("mixed/new.txt"), b"n").unwrap();
        let first = analyse_sources(
            std::slice::from_ref(&src),
            &nfc_plan(),
            None,
            None,
            |_, _, _| {},
        )
        .unwrap();
        let mut sent = SentBefore::new();
        for e in first.entries.iter().filter(|e| !e.is_dir) {
            let size = if e.raw_path.ends_with(b"changed.txt") {
                3
            } else {
                e.size
            };
            if !e.raw_path.ends_with(b"new.txt") {
                sent.insert((e.source, e.raw_path.clone()), (size, e.mtime));
            }
        }

        let a = analyse_sources(
            std::slice::from_ref(&src),
            &nfc_plan(),
            Some(&sent),
            None,
            |_, _, _| {},
        )
        .unwrap();
        let paths: Vec<_> = a
            .entries
            .iter()
            .map(|e| String::from_utf8_lossy(&e.raw_path).into_owned())
            .collect();
        assert_eq!(paths.len(), 4, "{paths:?}");
        assert_eq!(paths[0], "Acme");
        assert_eq!(paths[1], "Acme/mixed");
        assert!(paths.contains(&"Acme/mixed/changed.txt".to_owned()));
        assert!(paths.contains(&"Acme/mixed/new.txt".to_owned()));
        assert_eq!((a.files, a.dirs, a.unchanged), (2, 2, 2));
        // Renumbered: every parent is the kept folder, by its new index.
        assert!(a.entries[2..].iter().all(|e| e.parent_idx == Some(1)));
        assert_eq!(a.entries[1].parent_idx, Some(0));
        assert!(a.entries.iter().enumerate().all(|(i, e)| e.idx == i as u64));

        // Nothing changed: nothing at all, not even the root.
        let mut all = sent.clone();
        for e in a.entries.iter().filter(|e| !e.is_dir) {
            all.insert((e.source, e.raw_path.clone()), (e.size, e.mtime));
        }
        let none = analyse_sources(&[src], &nfc_plan(), Some(&all), None, |_, _, _| {}).unwrap();
        assert!(none.entries.is_empty());
        assert_eq!(none.unchanged, 4);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn undecodable_names_need_a_decision() {
        use filebridge_names::RULE_ENCODING;
        use std::os::unix::ffi::OsStrExt;
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("d");
        std::fs::create_dir(&src).unwrap();
        std::fs::write(src.join(OsStr::from_bytes(b"Patol\xf3gica.pdf")), b"x").unwrap();
        let a = analyse_sources(&[src], &nfc_plan(), None, None, |_, _, _| {}).unwrap();
        let undecodable: Vec<_> = a
            .issues
            .iter()
            .filter(|i| i.rule == RULE_ENCODING)
            .collect();
        assert_eq!(undecodable.len(), 1);
        assert_eq!(undecodable[0].before, b"Patol\xf3gica.pdf");
        assert_eq!(undecodable[0].severity, "needs_decision");
        assert_eq!(a.dest_paths[1].1, None);
    }
}
