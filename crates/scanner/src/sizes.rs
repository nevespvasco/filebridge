//! A folder's size, split by what is directly inside it (docs/design/specs/D02): each subfolder
//! with everything below it, and each file. Read-only, like the scan: metadata and listings
//! only, symbolic links neither followed nor counted.

use std::ffi::OsString;
use std::path::Path;

use walkdir::WalkDir;

/// One item directly inside the measured folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChildSize {
    pub name: OsString,
    pub is_dir: bool,
    /// Files inside (1 for a file).
    pub files: u64,
    pub bytes: u64,
}

/// What a measurement has counted so far.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SizeTally {
    /// In the order they were met (by name).
    pub children: Vec<ChildSize>,
    pub files: u64,
    /// Folders below the root.
    pub dirs: u64,
    pub bytes: u64,
    /// Entries that couldn't be read; not counted.
    pub errors: u64,
}

/// How often (in entries met) [`folder_sizes`] reports what it has counted.
pub const SIZES_REPORT_EVERY: u64 = 1024;

/// Measures `root`, a folder. `stop` is asked before each entry: once it says `true` the walk
/// ends and the answer is what was counted (`false` = not complete). `progress` sees the tally
/// every [`SIZES_REPORT_EVERY`] entries.
pub fn folder_sizes(
    root: &Path,
    stop: &dyn Fn() -> bool,
    progress: &mut dyn FnMut(&SizeTally),
) -> std::io::Result<(SizeTally, bool)> {
    let meta = std::fs::metadata(root)?;
    if !meta.is_dir() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotADirectory,
            format!("{} is not a directory", root.display()),
        ));
    }
    let mut tally = SizeTally::default();
    // The walk is depth first: everything below a child comes right after it.
    let mut current: Option<usize> = None;
    let mut met = 0u64;
    let walker = WalkDir::new(root)
        .min_depth(1)
        .follow_links(false)
        .sort_by(|a, b| a.file_name().cmp(b.file_name()));
    for item in walker {
        if stop() {
            return Ok((tally, false));
        }
        met += 1;
        if met.is_multiple_of(SIZES_REPORT_EVERY) {
            progress(&tally);
        }
        let item = match item {
            Ok(item) => item,
            Err(_) => {
                tally.errors += 1;
                continue;
            }
        };
        let ft = item.file_type();
        if ft.is_symlink() {
            if item.depth() == 1 {
                current = None;
            }
            continue;
        }
        let size = if ft.is_file() {
            match item.metadata() {
                Ok(m) => m.len(),
                Err(_) => {
                    tally.errors += 1;
                    if item.depth() == 1 {
                        current = None;
                    }
                    continue;
                }
            }
        } else {
            0
        };
        if item.depth() == 1 {
            current = Some(tally.children.len());
            tally.children.push(ChildSize {
                name: item.file_name().to_owned(),
                is_dir: ft.is_dir(),
                files: 0,
                bytes: 0,
            });
        }
        if ft.is_dir() {
            tally.dirs += 1;
        } else if ft.is_file() {
            tally.files += 1;
            tally.bytes += size;
            if let Some(c) = current.and_then(|i| tally.children.get_mut(i)) {
                c.files += 1;
                c.bytes += size;
            }
        }
    }
    progress(&tally);
    Ok((tally, true))
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::cell::Cell;

    fn tree() -> tempfile::TempDir {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join("Fotos/2019/verão")).unwrap();
        std::fs::create_dir_all(root.join("Vazia")).unwrap();
        std::fs::write(root.join("Fotos/2019/a.jpg"), vec![0u8; 100]).unwrap();
        std::fs::write(root.join("Fotos/2019/verão/b.jpg"), vec![0u8; 50]).unwrap();
        std::fs::write(root.join("Fotos/c.jpg"), vec![0u8; 7]).unwrap();
        std::fs::write(root.join("notas.txt"), vec![0u8; 3]).unwrap();
        std::os::unix::fs::symlink(root.join("Fotos"), root.join("atalho")).unwrap();
        tmp
    }

    #[test]
    fn each_child_holds_everything_below_it() {
        let tmp = tree();
        let (tally, complete) = folder_sizes(tmp.path(), &|| false, &mut |_| {}).unwrap();
        assert!(complete);
        let by_name = |n: &str| {
            tally
                .children
                .iter()
                .find(|c| c.name == n)
                .unwrap_or_else(|| panic!("{n} listed"))
                .clone()
        };
        let fotos = by_name("Fotos");
        assert_eq!((fotos.is_dir, fotos.files, fotos.bytes), (true, 3, 157));
        let notas = by_name("notas.txt");
        assert_eq!((notas.is_dir, notas.files, notas.bytes), (false, 1, 3));
        assert_eq!(by_name("Vazia").bytes, 0);
        assert!(
            tally.children.iter().all(|c| c.name != "atalho"),
            "links aren't followed or listed"
        );
        assert_eq!((tally.files, tally.bytes), (4, 160));
        assert_eq!(tally.dirs, 4, "Fotos, 2019, verão, Vazia");
        assert_eq!(tally.errors, 0);
    }

    #[test]
    fn stopping_answers_what_was_counted() {
        let tmp = tree();
        let asked = Cell::new(0);
        let stop = || {
            asked.set(asked.get() + 1);
            asked.get() > 2
        };
        let (tally, complete) = folder_sizes(tmp.path(), &stop, &mut |_| {}).unwrap();
        assert!(!complete);
        assert!(tally.files < 4, "{tally:?}");
    }

    #[test]
    fn a_file_is_not_a_folder_to_measure() {
        let tmp = tree();
        let err = folder_sizes(&tmp.path().join("notas.txt"), &|| false, &mut |_| {});
        assert!(err.is_err());
        assert!(folder_sizes(&tmp.path().join("nope"), &|| false, &mut |_| {}).is_err());
    }
}
