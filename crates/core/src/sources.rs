//! Chosen sources checked before anything is sent (docs/design/specs/D02): what each one is,
//! how big a file is, how many items a folder holds, what is wrong with it, and whether another
//! chosen source already contains it. Nothing is read beyond metadata and folder listings, and
//! nothing is written.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use filebridge_model::{FileCategory, SOURCE_CHILDREN_CAP, SourceInfo, SourceProblem};

use crate::analyse::os_bytes;
use crate::{CoreError, Engine, Result};

impl Engine {
    /// Checks each chosen source; the answer is in the same order as `paths`.
    pub async fn inspect_sources(&self, paths: Vec<String>) -> Result<Vec<SourceInfo>> {
        tokio::task::spawn_blocking(move || inspect(&paths))
            .await
            .map_err(|e| CoreError::Invalid(format!("source check failed: {e}")))
    }
}

/// See [`Engine::inspect_sources`].
pub(crate) fn inspect(paths: &[String]) -> Vec<SourceInfo> {
    let mut out: Vec<SourceInfo> = paths.iter().map(|p| one(p)).collect();
    // One with a problem is fixed on its own first; it neither covers nor is covered.
    let usable: Vec<Option<&Path>> = out
        .iter()
        .map(|i| i.problem.is_none().then_some(Path::new(&i.path)))
        .collect();
    let covered = covering(&usable);
    for (info, c) in out.iter_mut().zip(covered) {
        info.covered_by = c.map(|i| i as u32);
    }
    out
}

fn one(path: &str) -> SourceInfo {
    let mut info = SourceInfo {
        path: path.to_owned(),
        problem: None,
        is_dir: false,
        size: None,
        children: None,
        children_capped: false,
        mtime: None,
        category: None,
        covered_by: None,
        link_target: None,
    };
    let abs = match std::path::absolute(path) {
        Ok(a) => a,
        Err(_) => {
            info.problem = Some(SourceProblem::Missing);
            return info;
        }
    };
    let Some(name) = abs.file_name() else {
        info.problem = Some(SourceProblem::Root);
        return info;
    };
    let meta = match std::fs::symlink_metadata(&abs) {
        Ok(m) => m,
        Err(e) => {
            info.problem = Some(match e.kind() {
                std::io::ErrorKind::PermissionDenied => SourceProblem::Unreadable,
                _ => SourceProblem::Missing,
            });
            return info;
        }
    };
    info.mtime = filebridge_scanner::mtime_ms(&meta);
    let ft = meta.file_type();
    if ft.is_symlink() {
        info.problem = Some(SourceProblem::Symlink);
        info.link_target = std::fs::canonicalize(&abs)
            .ok()
            .map(|t| t.display().to_string());
        return info;
    }
    if ft.is_dir() {
        info.is_dir = true;
        match std::fs::read_dir(&abs) {
            Ok(entries) => {
                let n = entries.take(SOURCE_CHILDREN_CAP as usize + 1).count() as u32;
                info.children_capped = n > SOURCE_CHILDREN_CAP;
                info.children = Some(n.min(SOURCE_CHILDREN_CAP));
            }
            Err(_) => info.problem = Some(SourceProblem::Unreadable),
        }
    } else if ft.is_file() {
        info.size = Some(meta.len());
        info.category = Some(FileCategory::of_name(&os_bytes(name)));
        // Opened read-only and closed at once: only to know it can be read.
        if std::fs::File::open(&abs).is_err() {
            info.problem = Some(SourceProblem::Unreadable);
        }
    } else {
        info.problem = Some(SourceProblem::Special);
    }
    info
}

/// For each path, the index of the outermost other path that contains it (whole components),
/// or the earlier one of two that are the same. `None`s, paths that don't resolve and the root
/// are left out.
fn covering(paths: &[Option<&Path>]) -> Vec<Option<usize>> {
    let canon: Vec<Option<PathBuf>> = paths
        .iter()
        .map(|p| std::fs::canonicalize((*p)?).ok())
        .map(|c| c.filter(|c| c.parent().is_some()))
        .collect();
    let mut first: HashMap<&Path, usize> = HashMap::new();
    for (i, c) in canon.iter().enumerate() {
        if let Some(c) = c {
            first.entry(c.as_path()).or_insert(i);
        }
    }
    canon
        .iter()
        .enumerate()
        .map(|(i, c)| {
            let c = c.as_ref()?;
            // `ancestors` starts at the path itself: a duplicate's earlier copy counts too.
            let mut outermost = None;
            for a in c.ancestors() {
                if let Some(&j) = first.get(a)
                    && j != i
                {
                    outermost = Some(j);
                }
            }
            outermost
        })
        .collect()
}

/// The first chosen source that another one already contains: `(inside, container)`.
pub(crate) fn first_nested(paths: &[PathBuf]) -> Option<(PathBuf, PathBuf)> {
    let refs: Vec<Option<&Path>> = paths.iter().map(|p| Some(p.as_path())).collect();
    covering(&refs)
        .into_iter()
        .enumerate()
        .find_map(|(i, c)| c.map(|j| (paths[i].clone(), paths[j].clone())))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn covering(paths: &[String]) -> Vec<Option<usize>> {
        let refs: Vec<Option<&Path>> = paths.iter().map(|p| Some(Path::new(p))).collect();
        super::covering(&refs)
    }

    fn tree() -> (tempfile::TempDir, impl Fn(&str) -> String) {
        let dir = tempfile::tempdir().unwrap();
        for d in ["A/B/C", "foo", "foobar"] {
            std::fs::create_dir_all(dir.path().join(d)).unwrap();
        }
        let root = dir.path().to_path_buf();
        (dir, move |p: &str| root.join(p).display().to_string())
    }

    #[test]
    fn the_outermost_container_wins() {
        let (_d, p) = tree();
        let got = covering(&[p("A/B/C"), p("A/B"), p("A")]);
        assert_eq!(got, [Some(2), Some(2), None]);
    }

    #[test]
    fn a_duplicate_points_at_the_earlier_one() {
        let (_d, p) = tree();
        let got = covering(&[p("A"), p("A/B/../../A")]);
        assert_eq!(got, [None, Some(0)]);
    }

    #[test]
    fn a_name_prefix_is_not_containment() {
        let (_d, p) = tree();
        assert_eq!(covering(&[p("foo"), p("foobar")]), [None, None]);
    }

    #[test]
    fn the_root_covers_nothing() {
        let (_d, p) = tree();
        assert_eq!(covering(&["/".into(), p("A")]), [None, None]);
    }

    #[test]
    fn missing_paths_are_left_out() {
        let (_d, p) = tree();
        assert_eq!(covering(&[p("A"), p("nope")]), [None, None]);
    }
}
