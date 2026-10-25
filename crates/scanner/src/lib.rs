//! Walks the source and reads names as **bytes**, never assuming UTF-8.
//!
//! Read-only: the scanner never writes to, renames or deletes anything under the source.
//! A sequential walk producing [`names::Entry`] values, with each entry's modification time
//! alongside. Parallel walking and hashing land in later phases. Also: a folder's size split by
//! what is directly inside it ([`folder_sizes`]), and the disks mounted here ([`volumes`]).

mod sizes;
mod volumes;

pub use sizes::{ChildSize, SIZES_REPORT_EVERY, SizeTally, folder_sizes};
pub use volumes::{Volume, volumes};

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use filebridge_names::Entry;
use walkdir::WalkDir;

/// What a scan found.
#[derive(Debug, Default)]
pub struct Scan {
    /// Parents always come before their children.
    pub entries: Vec<Entry>,
    /// Modification time of each entry (Unix ms), same order as `entries`; `None` when the OS
    /// doesn't say. Used to tell whether a source changed after it was sent (D10).
    pub mtimes: Vec<Option<i64>>,
    /// Symlinks are not followed and not sent (Phase 0); listed here.
    pub skipped_symlinks: Vec<PathBuf>,
    /// Paths that could not be read (permissions, races), with the error.
    pub errors: Vec<(PathBuf, String)>,
}

impl Scan {
    pub fn files(&self) -> impl Iterator<Item = &Entry> {
        self.entries.iter().filter(|e| !e.is_dir)
    }

    pub fn total_bytes(&self) -> u64 {
        self.files().map(|e| e.size).sum()
    }
}

/// Scans everything below `root` (the root itself is not an entry).
pub fn scan(root: &Path) -> std::io::Result<Scan> {
    let meta = std::fs::metadata(root)?;
    if !meta.is_dir() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotADirectory,
            format!("{} is not a directory", root.display()),
        ));
    }

    let mut out = Scan::default();
    // dir_stack[d] = entry id of the directory at depth d + 1 on the current branch.
    let mut dir_stack: Vec<usize> = Vec::new();

    let walker = WalkDir::new(root)
        .min_depth(1)
        .follow_links(false)
        .sort_by(|a, b| a.file_name().cmp(b.file_name()));
    for item in walker {
        let item = match item {
            Ok(item) => item,
            Err(err) => {
                let path = err.path().map(Path::to_path_buf).unwrap_or_default();
                out.errors.push((path, err.to_string()));
                continue;
            }
        };
        let depth = item.depth();
        dir_stack.truncate(depth - 1);
        let ft = item.file_type();
        if ft.is_symlink() {
            out.skipped_symlinks.push(item.into_path());
            continue;
        }
        // One metadata read gives both the size and the modification time.
        let meta = match item.metadata() {
            Ok(m) => m,
            Err(err) => {
                out.errors.push((item.into_path(), err.to_string()));
                continue;
            }
        };
        let size = if ft.is_file() { meta.len() } else { 0 };
        let mtime = mtime_ms(&meta);
        let rel = item
            .path()
            .strip_prefix(root)
            .expect("walkdir yields paths under root");
        let parent = dir_stack.last().copied();
        let id = out.entries.len();
        out.entries.push(Entry::new(
            os_bytes(rel.as_os_str()),
            os_bytes(item.file_name()),
            size,
            ft.is_dir(),
            parent,
        ));
        out.mtimes.push(mtime);
        if ft.is_dir() {
            dir_stack.push(id);
        }
    }
    Ok(out)
}

/// A file's modification time in Unix milliseconds, when the OS gives one.
pub fn mtime_ms(meta: &std::fs::Metadata) -> Option<i64> {
    let t = meta.modified().ok()?;
    let d = t.duration_since(std::time::UNIX_EPOCH).ok()?;
    i64::try_from(d.as_millis()).ok()
}

#[cfg(unix)]
fn os_bytes(s: &OsStr) -> Vec<u8> {
    use std::os::unix::ffi::OsStrExt;
    s.as_bytes().to_vec()
}

#[cfg(not(unix))]
fn os_bytes(s: &OsStr) -> Vec<u8> {
    // Windows names are UTF-16 and cannot be invalid the same way; NFD still applies.
    s.to_string_lossy().replace('\\', "/").into_bytes()
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::ffi::OsStrExt;

    #[test]
    fn reads_nfd_and_invalid_names_as_bytes() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let nfd_dir = root.join(OsStr::from_bytes(b"Vera\xcc\x83o"));
        std::fs::create_dir(&nfd_dir).unwrap();
        std::fs::write(nfd_dir.join("praia.jpg"), b"12345").unwrap();
        // macOS file systems refuse a name that isn't UTF-8; NFD they keep as written.
        let invalid_names = !cfg!(target_os = "macos");
        if invalid_names {
            std::fs::write(root.join(OsStr::from_bytes(b"\xe7a.txt")), b"x").unwrap();
        }

        let scan = scan(root).unwrap();
        let by_path = |p: &[u8]| scan.entries.iter().position(|e| e.raw_path == p).unwrap();

        let dir = by_path(b"Vera\xcc\x83o");
        assert!(scan.entries[dir].is_dir);
        assert_eq!(scan.mtimes.len(), scan.entries.len());
        assert!(
            scan.mtimes.iter().all(Option::is_some),
            "the OS gives a time"
        );
        assert_eq!(scan.entries[dir].raw_name, b"Vera\xcc\x83o");

        let child = by_path(b"Vera\xcc\x83o/praia.jpg");
        assert_eq!(scan.entries[child].parent, Some(dir));
        assert_eq!(scan.entries[child].size, 5);

        if invalid_names {
            let latin1 = by_path(b"\xe7a.txt");
            assert_eq!(scan.entries[latin1].name, None);
        }
        assert_eq!(scan.total_bytes(), if invalid_names { 6 } else { 5 });
    }

    #[test]
    fn parents_come_before_children_and_symlinks_are_skipped() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join("a/b/c")).unwrap();
        std::fs::write(root.join("a/b/c/f"), b"").unwrap();
        std::fs::write(root.join("z"), b"").unwrap();
        std::os::unix::fs::symlink(root.join("z"), root.join("link")).unwrap();

        let scan = scan(root).unwrap();
        for (i, e) in scan.entries.iter().enumerate() {
            if let Some(p) = e.parent {
                assert!(p < i);
                assert!(scan.entries[p].is_dir);
                assert!(e.raw_path.starts_with(&scan.entries[p].raw_path));
            }
        }
        let z = scan.entries.iter().find(|e| e.raw_path == b"z").unwrap();
        assert_eq!(z.parent, None);
        assert_eq!(scan.skipped_symlinks.len(), 1);
    }
}
