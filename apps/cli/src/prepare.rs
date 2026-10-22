//! Scan + name rules → transfer items.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use filebridge_names::{Mapped, NamePlan, Nfc, RuleSet, TargetProfile};
use filebridge_scanner::Scan;
use filebridge_transport_api::{RemotePath, TransferItem};

pub struct Prepared {
    pub scan: Scan,
    pub mapped: Vec<Mapped>,
    pub items: Vec<TransferItem>,
    /// Files whose destination path differs from the source path (own name or an ancestor's).
    pub renamed_files: usize,
    /// Files skipped because a name on their path is not valid UTF-8 (`encoding` rule, Phase 2).
    pub unmappable: Vec<PathBuf>,
}

impl Prepared {
    pub fn files(&self) -> usize {
        self.scan.files().count()
    }

    pub fn dirs(&self) -> usize {
        self.scan.entries.len() - self.files()
    }

    pub fn renamed_nodes(&self) -> (usize, usize) {
        let mut files = 0;
        let mut dirs = 0;
        for (e, m) in self.scan.entries.iter().zip(&self.mapped) {
            if m.renamed() {
                if e.is_dir { dirs += 1 } else { files += 1 }
            }
        }
        (files, dirs)
    }
}

pub fn prepare(source: &Path) -> Result<Prepared> {
    let scan = filebridge_scanner::scan(source)
        .with_context(|| format!("cannot scan {}", source.display()))?;
    let target = TargetProfile::default();
    let names = NamePlan::new(
        RuleSet::new(vec![Box::new(Nfc)], &target),
        target.case_sensitive,
    );
    let mapped = names.propose(&scan.entries);
    let mut items = Vec::new();
    let mut unmappable = Vec::new();
    let mut renamed_files = 0;
    for (entry, m) in scan.entries.iter().zip(&mapped) {
        if entry.is_dir {
            continue;
        }
        let source = path_from_bytes(&entry.raw_path);
        match &m.dest_path {
            Some(dest) => {
                if dest.as_bytes() != entry.raw_path.as_slice() {
                    renamed_files += 1;
                }
                items.push(TransferItem {
                    root: 0,
                    source,
                    dest: RemotePath::new(dest.clone())?,
                    size: entry.size,
                    expected_hash: None,
                });
            }
            None => unmappable.push(source),
        }
    }
    Ok(Prepared {
        scan,
        mapped,
        items,
        renamed_files,
        unmappable,
    })
}

pub fn print_scan(source: &Path, show: bool) -> Result<()> {
    let p = prepare(source)?;
    let (rf, rd) = p.renamed_nodes();
    println!(
        "{}: {} files, {} dirs, {:.1} MB",
        source.display(),
        p.files(),
        p.dirs(),
        p.scan.total_bytes() as f64 / 1e6
    );
    println!(
        "nfc: {} names to correct ({rf} files, {rd} dirs); {} file paths change",
        rf + rd,
        p.renamed_files
    );
    println!(
        "non-UTF-8 names (skipped until Phase 2): {}",
        p.unmappable.len()
    );
    println!(
        "symlinks skipped: {}; unreadable: {}",
        p.scan.skipped_symlinks.len(),
        p.scan.errors.len()
    );
    if show {
        for (e, m) in p.scan.entries.iter().zip(&p.mapped) {
            for issue in &m.issues {
                println!(
                    "  [{}] {}\n      {} -> {}",
                    issue.rule,
                    String::from_utf8_lossy(&e.raw_path),
                    hex(&issue.before),
                    hex(&issue.after)
                );
            }
        }
    }
    Ok(())
}

fn hex(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(unix)]
pub fn path_from_bytes(bytes: &[u8]) -> PathBuf {
    use std::os::unix::ffi::OsStrExt;
    PathBuf::from(std::ffi::OsStr::from_bytes(bytes))
}

#[cfg(not(unix))]
pub fn path_from_bytes(bytes: &[u8]) -> PathBuf {
    PathBuf::from(String::from_utf8_lossy(bytes).into_owned())
}
