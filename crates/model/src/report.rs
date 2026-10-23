//! Comparing two reports and checking a sealed one (docs/design/specs/D09, ADR 0025).

use serde::{Deserialize, Serialize};

use crate::EntryStatus;

/// What changed for one file between two reports of the same source. A file can be in several
/// (renamed and newly verified).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum ReportChangeKind {
    /// Only in the second report.
    Added,
    /// Only in the first report.
    Removed,
    /// Its size or its SHA-256 differs.
    Changed,
    /// It went to another destination path (a name rule, or another target folder).
    Renamed,
    /// Verified in the second report and not in the first.
    NewlyVerified,
    /// A problem (different, missing, unverifiable, failed) in the second report and not in the
    /// first.
    NewProblem,
}

impl ReportChangeKind {
    pub const ALL: [ReportChangeKind; 6] = [
        Self::Added,
        Self::Removed,
        Self::Changed,
        Self::Renamed,
        Self::NewlyVerified,
        Self::NewProblem,
    ];

    /// The stable word for scripts (English, whatever the UI language).
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Added => "added",
            Self::Removed => "removed",
            Self::Changed => "changed",
            Self::Renamed => "renamed",
            Self::NewlyVerified => "newly_verified",
            Self::NewProblem => "new_problem",
        }
    }
}

/// One file as one of the two reports has it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub struct ReportSide {
    pub destination_path: Option<String>,
    pub size: u64,
    pub status: EntryStatus,
    pub sha256: Option<String>,
    /// The name rules that renamed it.
    pub renamed_by: Vec<String>,
}

/// One file that differs between the two reports, matched by its source path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub struct ReportChange {
    /// Relative to the source's parent, lossily decoded.
    pub source_path: String,
    pub kinds: Vec<ReportChangeKind>,
    /// `None` when added.
    pub a: Option<ReportSide>,
    /// `None` when removed.
    pub b: Option<ReportSide>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub struct ReportChangeCount {
    pub kind: ReportChangeKind,
    pub files: u64,
}

/// Two jobs' reports side by side: what changed from the first to the second.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub struct ReportDiff {
    /// Every kind, in [`ReportChangeKind::ALL`] order, zeros included; over the whole reports.
    pub counts: Vec<ReportChangeCount>,
    /// Files in both reports with no change.
    pub unchanged: u64,
    /// In source-path order: the first `limit` changes (of `kind` when one was asked for).
    pub changes: Vec<ReportChange>,
    /// More changes than listed.
    pub more: bool,
    /// Both jobs have the same source folders; when not, matching paths may be other files.
    pub same_sources: bool,
    pub same_profile: bool,
}

/// What a sealed report's check found wrong.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SealProblem {
    /// Not a filebridge report, or not laid out as filebridge writes one.
    Unreadable { detail: String },
    /// A report exported without a seal: nothing to check.
    NotSealed,
    /// The summary at the top (transfer, destination, totals) was changed.
    Header,
    /// This entry (0-based, in file order) was changed, or one was removed or added just
    /// before it.
    Entry { index: u64, source_path: String },
    /// Entries were removed or added at the end, or the seal itself was changed.
    Seal,
    /// The readable copy (HTML) doesn't match the sealed JSON.
    Readable,
}

/// The answer of `filebridge report check` (ADR 0025).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub struct ReportCheck {
    /// Nothing changed since the export.
    pub intact: bool,
    /// The sealed JSON checked.
    pub path: String,
    /// The readable copy beside it, when there is one; it is checked too.
    pub readable: Option<String>,
    pub entries: u64,
    /// The final SHA-256 the seal records (compare it with a copy kept elsewhere).
    pub sha256: Option<String>,
    /// The first problems found (at most 50 entries).
    pub problems: Vec<SealProblem>,
    /// Changed entries beyond those listed.
    pub more: u64,
}
