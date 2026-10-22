//! Stages 2 and 3 for a set of sources: scan (names as bytes), run the name rules, and find
//! destination collisions. Synchronous; the engine runs it on a blocking thread.
//!
//! Each source root is itself an entry, so sending `/home/u/Acme` creates `Acme/…` at the
//! destination and the root's own name goes through the rules too. Paths in the manifest are
//! relative to the source's parent directory.

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

pub(crate) fn analyse_sources(
    sources: &[PathBuf],
    names: &NamePlan,
    mut on_source: impl FnMut(&Path, u64, u64),
) -> Result<Analysis> {
    let mut entries: Vec<Entry> = Vec::new();
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
        source_of.push(si as u32);
        let (mut files, mut bytes) = (u64::from(!meta.is_dir()), size);
        if meta.is_dir() {
            let scan = filebridge_scanner::scan(source)?;
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

    let mapped = names.propose(&entries);

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
            mtime: None,
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

    Ok(Analysis {
        entries: out_entries,
        dest_paths,
        issues,
        files,
        dirs,
        bytes,
        scan_errors,
        skipped_symlinks,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use filebridge_names::{Nfc, RULE_COLLISION, RULE_ENCODING, RuleSet, TargetProfile};

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
        let a = analyse_sources(std::slice::from_ref(&src), &nfc_plan(), |p, f, b| {
            seen.push((p.to_path_buf(), f, b))
        })
        .unwrap();
        assert_eq!(seen, vec![(src, 3, 6)]);
        assert_eq!((a.files, a.dirs, a.bytes), (3, 2, 6));
        assert_eq!(a.entries[0].raw_path, b"Acme");
        assert_eq!(a.entries[0].parent_idx, None);
        assert!(a.entries[1..].iter().all(|e| e.parent_idx.is_some()));
        let paths: Vec<_> = a.dest_paths.iter().filter_map(|(_, p)| p.clone()).collect();
        assert!(paths.contains(&"Acme/sub/a.txt".to_owned()));
        assert_eq!(paths.iter().filter(|p| *p == "Acme/Verão.txt").count(), 2);

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

    #[cfg(target_os = "linux")]
    #[test]
    fn undecodable_names_need_a_decision() {
        use std::os::unix::ffi::OsStrExt;
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("d");
        std::fs::create_dir(&src).unwrap();
        std::fs::write(src.join(OsStr::from_bytes(b"Patol\xf3gica.pdf")), b"x").unwrap();
        let a = analyse_sources(&[src], &nfc_plan(), |_, _, _| {}).unwrap();
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
