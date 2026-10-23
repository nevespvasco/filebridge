//! Comparing two reports of the same source: each file matched by its source path, then what
//! changed for it (docs/design/specs/D09, ADR 0025). Both sides are walked in source-path order,
//! so two 300 000-entry jobs are compared a page at a time.

use std::cmp::Ordering;

use filebridge_model::{
    EntryStatus, ReportChange, ReportChangeCount, ReportChangeKind, ReportDiff, ReportSide,
};
use serde::Deserialize;

use super::ReportRow;

/// Which side(s) to take next, given each side's next key: `None` once both are done. Equal keys
/// are the same file; otherwise the smaller key is only on its side.
pub(crate) fn step(a: Option<&[u8]>, b: Option<&[u8]>) -> Option<(bool, bool)> {
    match (a, b) {
        (None, None) => None,
        (Some(_), None) => Some((true, false)),
        (None, Some(_)) => Some((false, true)),
        (Some(x), Some(y)) => Some(match x.cmp(y) {
            Ordering::Less => (true, false),
            Ordering::Greater => (false, true),
            Ordering::Equal => (true, true),
        }),
    }
}

fn status(r: &ReportRow) -> EntryStatus {
    EntryStatus::parse(&r.status).unwrap_or(EntryStatus::NotSent)
}

/// What changed for one file from `a` to `b` (empty: nothing).
pub fn kinds(a: Option<&ReportRow>, b: Option<&ReportRow>) -> Vec<ReportChangeKind> {
    use ReportChangeKind::*;
    let (a, b) = match (a, b) {
        (None, None) => return Vec::new(),
        (None, Some(_)) => return vec![Added],
        (Some(_), None) => return vec![Removed],
        (Some(a), Some(b)) => (a, b),
    };
    let mut out = Vec::new();
    let hashes_differ = matches!((&a.sha256, &b.sha256), (Some(x), Some(y)) if x != y);
    if a.size != b.size || hashes_differ {
        out.push(Changed);
    }
    if a.destination_path.is_some()
        && b.destination_path.is_some()
        && a.destination_path != b.destination_path
    {
        out.push(Renamed);
    }
    let (sa, sb) = (status(a), status(b));
    if sb == EntryStatus::Verified && sa != EntryStatus::Verified {
        out.push(NewlyVerified);
    }
    if sb.is_problem() && !sa.is_problem() {
        out.push(NewProblem);
    }
    out
}

fn side(r: ReportRow) -> ReportSide {
    ReportSide {
        status: status(&r),
        destination_path: r.destination_path,
        size: r.size,
        sha256: r.sha256,
        renamed_by: r.renamed_by,
    }
}

/// Counts the changes of the pairs it is given and hands each change to `each`.
pub struct Differ<F> {
    each: F,
    counts: [u64; ReportChangeKind::ALL.len()],
    unchanged: u64,
}

impl<F: FnMut(ReportChange)> Differ<F> {
    pub fn new(each: F) -> Self {
        Self {
            each,
            counts: [0; ReportChangeKind::ALL.len()],
            unchanged: 0,
        }
    }

    /// One file: in the first report, the second, or both.
    pub fn pair(&mut self, a: Option<ReportRow>, b: Option<ReportRow>) {
        let kinds = kinds(a.as_ref(), b.as_ref());
        if kinds.is_empty() {
            if a.is_some() {
                self.unchanged += 1;
            }
            return;
        }
        for k in &kinds {
            let i = ReportChangeKind::ALL
                .iter()
                .position(|x| x == k)
                .expect("every kind is in ALL");
            self.counts[i] += 1;
        }
        let source_path = b
            .as_ref()
            .or(a.as_ref())
            .map(|r| r.source_path.clone())
            .unwrap_or_default();
        (self.each)(ReportChange {
            source_path,
            kinds,
            a: a.map(side),
            b: b.map(side),
        });
    }

    pub fn finish(self) -> ReportDiff {
        ReportDiff {
            counts: ReportChangeKind::ALL
                .into_iter()
                .zip(self.counts)
                .map(|(kind, files)| ReportChangeCount { kind, files })
                .collect(),
            unchanged: self.unchanged,
            changes: Vec::new(),
            more: false,
            same_sources: true,
            same_profile: true,
        }
    }
}

/// Compares two reports' files held in memory (read from exported files); directories are left
/// out, as in a comparison of two jobs. `same_sources` and `same_profile` are left `true`.
pub fn diff_rows(
    a: Vec<ReportRow>,
    b: Vec<ReportRow>,
    each: impl FnMut(ReportChange),
) -> ReportDiff {
    let sorted = |rows: Vec<ReportRow>| {
        let mut keyed: Vec<_> = rows
            .into_iter()
            .filter(|r| !r.is_dir)
            .map(|r| (r.key(), r))
            .collect();
        keyed.sort_by(|x, y| x.0.cmp(&y.0));
        keyed.into_iter().peekable()
    };
    let (mut a, mut b) = (sorted(a), sorted(b));
    let mut d = Differ::new(each);
    while let Some((ta, tb)) = step(
        a.peek().map(|r| r.0.as_slice()),
        b.peek().map(|r| r.0.as_slice()),
    ) {
        let ra = if ta { a.next().map(|r| r.1) } else { None };
        let rb = if tb { b.next().map(|r| r.1) } else { None };
        d.pair(ra, rb);
    }
    d.finish()
}

/// An exported JSON report (plain or sealed): its transfer summary and its entries.
#[derive(Debug, Deserialize)]
pub struct ReportFile {
    pub format: String,
    pub transfer: serde_json::Value,
    pub entries: Vec<ReportRow>,
}

/// Reads a JSON report written by `export_report` or `filebridge report --format json|sealed`.
pub fn read_report(path: &std::path::Path) -> std::io::Result<ReportFile> {
    let file = std::io::BufReader::new(std::fs::File::open(path)?);
    let report: ReportFile = serde_json::from_reader(file)?;
    if !report.format.starts_with("filebridge-report/") {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("{} is not a filebridge report", path.display()),
        ));
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(path: &str, dest: &str, size: u64, status: &str, sha: Option<&str>) -> ReportRow {
        ReportRow {
            source_path: path.into(),
            source_path_hex: None,
            destination_path: Some(dest.into()),
            is_dir: false,
            size,
            mtime: None,
            category: "other".into(),
            status: status.into(),
            renamed_by: Vec::new(),
            sha256: sha.map(str::to_owned),
            error: None,
            chain: None,
        }
    }

    #[test]
    fn each_kind_of_change_is_found() {
        use ReportChangeKind::*;
        let a = vec![
            row("same.txt", "same.txt", 1, "verified", Some("aa")),
            row("gone.txt", "gone.txt", 1, "verified", None),
            row("edited.txt", "edited.txt", 1, "verified", Some("aa")),
            row("a:b.txt", "a:b.txt", 1, "sent", None),
            row("broke.txt", "broke.txt", 1, "verified", None),
        ];
        let b = vec![
            row("new.txt", "new.txt", 1, "sent", None),
            row("broke.txt", "broke.txt", 1, "different", None),
            row("a:b.txt", "a_b.txt", 1, "verified", None),
            row("edited.txt", "edited.txt", 1, "verified", Some("bb")),
            row("same.txt", "same.txt", 1, "verified", Some("aa")),
        ];
        let mut seen = Vec::new();
        let d = diff_rows(a, b, |c| seen.push((c.source_path, c.kinds)));
        assert_eq!(
            seen,
            [
                ("a:b.txt".to_owned(), vec![Renamed, NewlyVerified]),
                ("broke.txt".to_owned(), vec![NewProblem]),
                ("edited.txt".to_owned(), vec![Changed]),
                ("gone.txt".to_owned(), vec![Removed]),
                ("new.txt".to_owned(), vec![Added]),
            ],
            "in source-path order"
        );
        assert_eq!(d.unchanged, 1);
        let count = |k| d.counts.iter().find(|c| c.kind == k).unwrap().files;
        assert_eq!(
            [Added, Removed, Changed, Renamed, NewlyVerified, NewProblem].map(count),
            [1, 1, 1, 1, 1, 1]
        );
    }

    #[test]
    fn a_name_that_is_not_text_is_matched_by_its_bytes() {
        let mut a = row("caf\u{fffd}", "cafe", 1, "sent", None);
        a.source_path_hex = Some("636166e9".into());
        let mut other = a.clone();
        other.source_path_hex = Some("636166ea".into());
        let d = diff_rows(vec![a.clone()], vec![a.clone(), other], |_| {});
        assert_eq!(d.unchanged, 1);
        assert_eq!(d.counts[0].files, 1, "the other byte is another file");
    }
}
