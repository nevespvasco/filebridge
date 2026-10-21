//! Plan B rename planning: which nodes of a tree copied with original names must be renamed,
//! and in which order, to reach the corrected names.

use std::collections::{BTreeMap, BTreeSet};

use filebridge_transport_api::{Result, TransportError};

/// Renames relative to the temp root. Every `from`/`to` pair shares the same parent, written
/// with **original** names: files are renamed first, then directories deepest-first, so no
/// ancestor has been renamed yet when a node is.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct RenamePlan {
    pub files: Vec<(String, String)>,
    /// Directory renames grouped by depth (0 = top level); apply in reverse order.
    pub dirs_by_depth: BTreeMap<usize, Vec<(String, String)>>,
}

impl RenamePlan {
    pub fn len(&self) -> usize {
        self.files.len() + self.dirs_by_depth.values().map(Vec::len).sum::<usize>()
    }
}

/// `pairs` are (original source path, corrected destination path), both `/`-separated.
pub(crate) fn plan<'a>(pairs: impl IntoIterator<Item = (&'a str, &'a str)>) -> Result<RenamePlan> {
    let mut files = BTreeSet::new();
    let mut dirs: BTreeMap<usize, BTreeSet<(String, String)>> = BTreeMap::new();
    for (src, dst) in pairs {
        let s: Vec<&str> = src.split('/').collect();
        let d: Vec<&str> = dst.split('/').collect();
        if s.len() != d.len() {
            return Err(TransportError::fatal(format!(
                "rename changes path depth: {src:?} -> {dst:?}"
            )));
        }
        for k in 0..s.len() {
            if s[k] == d[k] {
                continue;
            }
            let parent = s[..k].join("/");
            let from = crate::fs::join(&parent, s[k]);
            let to = crate::fs::join(&parent, d[k]);
            if k + 1 == s.len() {
                files.insert((from, to));
            } else {
                dirs.entry(k).or_default().insert((from, to));
            }
        }
    }
    Ok(RenamePlan {
        files: files.into_iter().collect(),
        dirs_by_depth: dirs
            .into_iter()
            .map(|(k, v)| (k, v.into_iter().collect()))
            .collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn files_then_dirs_with_original_parents() {
        let nfd_dir = "Vera\u{303}o";
        let nfc_dir = "Ver\u{e3}o";
        let src1 = format!("Fotos/{nfd_dir}/Cafe\u{301}.jpg");
        let dst1 = format!("Fotos/{nfc_dir}/Caf\u{e9}.jpg");
        let src2 = format!("Fotos/{nfd_dir}/ok.jpg");
        let dst2 = format!("Fotos/{nfc_dir}/ok.jpg");
        let plan = plan([
            (src1.as_str(), dst1.as_str()),
            (src2.as_str(), dst2.as_str()),
            ("a", "a"),
        ])
        .unwrap();
        assert_eq!(
            plan.files,
            vec![(src1.clone(), format!("Fotos/{nfd_dir}/Caf\u{e9}.jpg"))],
            "file renamed inside its original-named parent"
        );
        assert_eq!(
            plan.dirs_by_depth.get(&1).unwrap(),
            &vec![(format!("Fotos/{nfd_dir}"), format!("Fotos/{nfc_dir}"))],
            "directory renamed once, even with two children"
        );
        assert_eq!(plan.len(), 2);
    }

    #[test]
    fn depth_change_is_rejected() {
        assert!(plan([("a/b", "a")]).is_err());
    }
}
