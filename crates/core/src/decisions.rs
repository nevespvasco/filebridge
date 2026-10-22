//! Name decisions as the store keeps them, translated to and from the Name plan's terms
//! (`filebridge_names`). The rules about what a decision means live there; this is only the
//! mapping.

use filebridge_names::{Choice, Decided, Entry, NameError};
use filebridge_store::{Decision, IssueRow, ManifestEntry};

use crate::{CoreError, DecisionInput, IssueDecision, IssueSeverity};

/// What the user decided, in the Name plan's terms.
pub(crate) fn choice(input: DecisionInput) -> Choice {
    match input {
        DecisionInput::Accept => Choice::Accept,
        DecisionInput::Edit(name) => Choice::Edit(name),
        DecisionInput::KeepOriginal => Choice::KeepOriginal,
        DecisionInput::Skip => Choice::Skip,
    }
}

impl From<NameError> for CoreError {
    fn from(e: NameError) -> Self {
        match e {
            NameError::Pending(n) => CoreError::PendingDecisions(n),
            other => CoreError::Invalid(other.to_string()),
        }
    }
}

/// How a choice is stored: the decision and, for an edit, the custom name.
pub(crate) fn stored(choice: Choice) -> (Decision, Option<String>) {
    match choice {
        Choice::Accept => (Decision::Accepted, None),
        Choice::KeepOriginal => (Decision::Rejected, None),
        Choice::Edit(name) => (Decision::Edited, Some(name)),
        Choice::Skip => (Decision::Skipped, None),
    }
}

pub(crate) fn decided(row: IssueRow) -> Decided {
    let choice = match row.issue.decision {
        Decision::Pending => None,
        Decision::Accepted => Some(Choice::Accept),
        Decision::Rejected => Some(Choice::KeepOriginal),
        Decision::Edited => Some(Choice::Edit(row.custom_name.unwrap_or_default())),
        Decision::Skipped => Some(Choice::Skip),
    };
    Decided {
        entry: row.issue.entry_idx as usize,
        rule: row.issue.rule,
        before: row.issue.before,
        after: row.issue.after,
        choice,
    }
}

/// A manifest entry as the name rules see it.
pub(crate) fn entry(e: &ManifestEntry) -> Entry {
    let raw = &e.entry.raw_path;
    let name = raw.rsplit(|b| *b == b'/').next().unwrap_or(raw).to_vec();
    Entry::new(
        raw.clone(),
        name,
        e.entry.size,
        e.entry.is_dir,
        e.entry.parent_idx.map(|p| p as usize),
    )
}

/// A stored decision as the UI sees it. Spelled out, so renaming a store value can't change the
/// wire silently.
pub(crate) fn view(decision: Decision) -> IssueDecision {
    match decision {
        Decision::Pending => IssueDecision::Pending,
        Decision::Accepted => IssueDecision::Accepted,
        Decision::Edited => IssueDecision::Edited,
        Decision::Rejected => IssueDecision::Rejected,
        Decision::Skipped => IssueDecision::Skipped,
    }
}

/// A stored severity (`auto` / `needs_decision`) as the UI sees it.
pub(crate) fn severity(stored: &str) -> IssueSeverity {
    if stored == "auto" {
        IssueSeverity::Auto
    } else {
        IssueSeverity::NeedsDecision
    }
}
