//! A job's files as the Files tab shows them (docs/design/specs/D05-files-and-entry-detail.md):
//! one word per entry for where it stands, the filter and order of a page, and an entry's detail.

use serde::{Deserialize, Serialize};

use crate::{CategoryCount, FileCategory, IssueView};

/// Where an entry stands, merging its transfer and verify states into one word.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum EntryStatus {
    NotSent,
    Sending,
    Sent,
    Verified,
    Different,
    Missing,
    Unverifiable,
    Failed,
    Skipped,
}

impl EntryStatus {
    pub const ALL: [EntryStatus; 9] = [
        Self::NotSent,
        Self::Sending,
        Self::Sent,
        Self::Verified,
        Self::Different,
        Self::Missing,
        Self::Unverifiable,
        Self::Failed,
        Self::Skipped,
    ];

    /// From the manifest's transfer and verify states (as stored).
    pub fn from_states(transfer: &str, verify: &str) -> Self {
        match (transfer, verify) {
            ("partial", _) => Self::Sending,
            ("failed", _) => Self::Failed,
            ("skipped", _) => Self::Skipped,
            ("done", "done") => Self::Verified,
            ("done", "mismatch") => Self::Different,
            ("done", "missing") => Self::Missing,
            ("done", "failed") => Self::Unverifiable,
            ("done", _) => Self::Sent,
            _ => Self::NotSent,
        }
    }

    /// The stored states this status covers: `(transfer, verify)`, `None` = any verify state.
    pub fn states(self) -> (&'static str, Option<&'static str>) {
        match self {
            Self::NotSent => ("pending", None),
            Self::Sending => ("partial", None),
            Self::Sent => ("done", Some("pending")),
            Self::Verified => ("done", Some("done")),
            Self::Different => ("done", Some("mismatch")),
            Self::Missing => ("done", Some("missing")),
            Self::Unverifiable => ("done", Some("failed")),
            Self::Failed => ("failed", None),
            Self::Skipped => ("skipped", None),
        }
    }

    /// The stable word for reports and scripts (English, whatever the UI language).
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NotSent => "not_sent",
            Self::Sending => "sending",
            Self::Sent => "sent",
            Self::Verified => "verified",
            Self::Different => "different",
            Self::Missing => "missing",
            Self::Unverifiable => "unverifiable",
            Self::Failed => "failed",
            Self::Skipped => "skipped",
        }
    }

    /// Different, missing, couldn't verify or failed: what needs a look.
    pub fn is_problem(self) -> bool {
        matches!(
            self,
            Self::Different | Self::Missing | Self::Unverifiable | Self::Failed
        )
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum EntrySort {
    /// Scanner order: folders read as a tree.
    #[default]
    FolderOrder,
    LargestFirst,
    ProblemsFirst,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub struct EntryFilter {
    /// Any of these; empty = any.
    pub statuses: Vec<EntryStatus>,
    /// Any of these; empty = any.
    pub categories: Vec<FileCategory>,
    /// Only renamed entries (`true`), only the others (`false`).
    pub renamed: Option<bool>,
    pub include_dirs: bool,
    /// A substring of the destination or source path.
    pub query: Option<String>,
    pub sort: EntrySort,
}

/// Where a page ended; pass it back to get the next one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub struct EntryCursor {
    pub idx: u64,
    pub size: u64,
    pub rank: u8,
}

/// One row of the Files tab.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub struct EntryView {
    pub idx: u64,
    pub is_dir: bool,
    /// Destination name (the source name when it has none yet).
    pub name: String,
    /// Destination folder, relative to the profile root ("" at the root).
    pub dir: String,
    /// The source name, when a name rule changed it.
    pub source_name: Option<String>,
    pub size: u64,
    pub category: FileCategory,
    pub status: EntryStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub struct EntryPage {
    pub items: Vec<EntryView>,
    /// `None` when this was the last page.
    pub next: Option<EntryCursor>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub struct StatusCount {
    pub status: EntryStatus,
    pub files: u64,
}

/// Counts for the filter chips: the whole job, whatever the current filter.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub struct EntryFacets {
    pub by_status: Vec<StatusCount>,
    pub by_category: Vec<CategoryCount>,
    pub renamed: u64,
    pub problems: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub struct HashView {
    pub algo: String,
    pub hex: String,
}

/// Everything about one entry, for the detail drawer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type)]
pub struct EntryDetail {
    pub view: EntryView,
    /// Absolute, lossily decoded.
    pub source_path: String,
    /// The raw bytes in hex, when the name isn't valid text (so the lossy text isn't exact).
    pub source_path_hex: Option<String>,
    /// The profile root joined with the destination path.
    pub dest_path: Option<String>,
    /// Unix seconds, when the scan recorded it.
    pub mtime: Option<i64>,
    /// The source file's digest, kept by verification.
    pub hash: Option<HashView>,
    /// The name rules that looked at this entry, with their decisions.
    pub issues: Vec<IssueView>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum ReportFormat {
    #[default]
    Csv,
    Json,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum ReportScope {
    #[default]
    All,
    Problems,
    Renamed,
}

/// What to write in a job's report, and where (docs/design/specs/D09).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub struct ReportRequest {
    pub format: ReportFormat,
    pub scope: ReportScope,
    pub include_dirs: bool,
    /// A file on the service's machine (the window's, for the desktop app).
    pub path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub struct ExportResult {
    pub path: String,
    pub rows: u64,
    pub bytes: u64,
}

#[cfg(test)]
mod tests {
    use super::EntryStatus;

    #[test]
    fn every_status_round_trips_through_its_states() {
        for s in EntryStatus::ALL {
            let (transfer, verify) = s.states();
            assert_eq!(
                EntryStatus::from_states(transfer, verify.unwrap_or("pending")),
                s,
                "{s:?}"
            );
        }
        assert!(EntryStatus::Missing.is_problem());
        assert!(!EntryStatus::Skipped.is_problem());
    }
}
