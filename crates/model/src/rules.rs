//! The Job lifecycle's rules, as pure data: which operation may run in which state, and which
//! state may follow which (docs/02, "Job state machine"). `core`'s `lifecycle.rs` enforces them
//! (locking, persisting); the UI reads the operation table (`JOB_OPERATIONS`).

use crate::JobState;

/// What a caller wants to do with a job.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    SetOptions,
    Analyse,
    Decide,
    Start,
    Pause,
    Cancel,
    /// Cancel an unsent job the New-transfer wizard replaced (ADR 0030): Cancelled, and hidden
    /// from the Transfers list.
    DiscardDraft,
    /// The runner moving a job through stages 4–6.
    Run,
    /// Startup: picking up a job the app left unfinished.
    Recover,
    /// Name a job (any state; changes no state).
    SetLabel,
    /// A new draft from a finished job's sources (Send again).
    SendAgain,
    /// Write a finished job's report.
    Export,
    /// Leave one file out of a send that has started (docs/design/specs/D04): while the
    /// transfer stage runs, waits or is paused.
    SkipEntry,
}

impl Op {
    /// Whether this operation may run on a job in `state`.
    pub fn allowed_in(self, state: JobState) -> bool {
        use JobState::*;
        match self {
            Op::SetOptions | Op::Analyse | Op::DiscardDraft => {
                matches!(state, Draft | AwaitingApproval | Ready)
            }
            Op::Decide => matches!(state, AwaitingApproval | Ready),
            Op::Start => matches!(state, Ready | Paused),
            Op::Pause | Op::Run => state.is_running(),
            Op::Cancel | Op::Recover => !state.is_terminal(),
            Op::SetLabel => true,
            Op::SendAgain | Op::Export => state.is_terminal(),
            Op::SkipEntry => matches!(state, Transferring | Retrying | Waiting | Paused),
        }
    }
}

/// The operations a user can ask for, and the states each is allowed in, keyed by the ipc
/// operation name. Exported to the UI (`JOB_OPERATIONS`) so it never keeps its own copy.
pub fn job_operations() -> std::collections::BTreeMap<&'static str, Vec<JobState>> {
    [
        ("set_options", Op::SetOptions),
        ("analyse", Op::Analyse),
        ("decide", Op::Decide),
        ("decide_all", Op::Decide),
        ("start", Op::Start),
        ("resume", Op::Start),
        ("pause", Op::Pause),
        ("cancel", Op::Cancel),
        ("discard_draft", Op::DiscardDraft),
        ("set_label", Op::SetLabel),
        ("send_again", Op::SendAgain),
        ("export_report", Op::Export),
        ("skip_entry", Op::SkipEntry),
    ]
    .into_iter()
    .map(|(name, op)| {
        let states = JobState::ALL
            .into_iter()
            .filter(|s| op.allowed_in(*s))
            .collect();
        (name, states)
    })
    .collect()
}

/// Which set a job's state falls in, for the user (docs/design/specs/D06, "Job groups").
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize, specta::Type,
)]
#[serde(rename_all = "snake_case")]
pub enum JobGroup {
    Active,
    NeedsYou,
    NotSent,
    Finished,
    Cancelled,
}

impl JobGroup {
    pub const ALL: [JobGroup; 5] = [
        Self::Active,
        Self::NeedsYou,
        Self::NotSent,
        Self::Finished,
        Self::Cancelled,
    ];

    pub fn of(state: JobState) -> Self {
        use JobState::*;
        match state {
            Analysing | Transferring | Retrying | Waiting | Verifying | Integrating | Paused => {
                Self::Active
            }
            AwaitingApproval | Failed => Self::NeedsYou,
            Draft | Ready => Self::NotSent,
            Completed | CompletedWithWarnings => Self::Finished,
            Cancelled => Self::Cancelled,
        }
    }

    pub fn states(self) -> Vec<JobState> {
        JobState::ALL
            .into_iter()
            .filter(|s| Self::of(*s) == self)
            .collect()
    }
}

/// Whether a job may go from `from` to `to`.
pub fn can_go(from: JobState, to: JobState) -> bool {
    use JobState::*;
    if from.is_terminal() {
        return false;
    }
    if matches!(to, Cancelled | Failed) {
        return true;
    }
    match from {
        Draft => matches!(to, Analysing),
        Analysing => matches!(to, AwaitingApproval | Ready | Draft),
        AwaitingApproval => matches!(to, Ready | Analysing),
        Ready => matches!(to, Transferring | Analysing | AwaitingApproval),
        Transferring => matches!(to, Paused | Retrying | Waiting | Verifying | Integrating),
        Paused => matches!(to, Transferring),
        Retrying => matches!(
            to,
            Transferring | Waiting | Verifying | Integrating | Paused
        ),
        Waiting => matches!(to, Transferring | Paused),
        Verifying => matches!(to, Transferring | Integrating | Retrying | Paused),
        Integrating => matches!(to, Completed | CompletedWithWarnings | Retrying | Paused),
        Completed | CompletedWithWarnings | Cancelled | Failed => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use JobState::*;

    #[test]
    fn transitions() {
        assert!(can_go(Draft, Analysing));
        assert!(!can_go(Draft, Transferring));
        assert!(can_go(Transferring, Retrying));
        for s in JobState::ALL {
            let groups = JobGroup::ALL
                .iter()
                .filter(|g| g.states().contains(&s))
                .count();
            assert_eq!(groups, 1, "{s:?} is in exactly one group");
        }
        assert!(can_go(Retrying, Transferring));
        assert!(can_go(Verifying, Transferring), "mismatches resend");
        assert!(can_go(Transferring, Waiting), "the send window closed");
        assert!(can_go(Waiting, Transferring), "the send window opened");
        assert!(!can_go(Waiting, Verifying), "only the transfer waits");
        assert!(can_go(Integrating, CompletedWithWarnings));
        assert!(can_go(Paused, Cancelled));
        assert!(!can_go(Completed, Cancelled), "terminal is final");
        assert!(!can_go(Failed, Transferring));
    }

    /// Every operation, in every state: the whole table in one place.
    #[test]
    fn operations_by_state() {
        let allowed = |op: Op| -> Vec<JobState> {
            JobState::ALL
                .into_iter()
                .filter(|s| op.allowed_in(*s))
                .collect()
        };
        assert_eq!(allowed(Op::SetOptions), [Draft, AwaitingApproval, Ready]);
        assert_eq!(allowed(Op::Analyse), [Draft, AwaitingApproval, Ready]);
        assert_eq!(allowed(Op::DiscardDraft), [Draft, AwaitingApproval, Ready]);
        assert_eq!(allowed(Op::Decide), [AwaitingApproval, Ready]);
        assert_eq!(allowed(Op::Start), [Ready, Paused]);
        assert_eq!(
            allowed(Op::Pause),
            [Transferring, Retrying, Waiting, Verifying, Integrating]
        );
        assert_eq!(allowed(Op::Run), allowed(Op::Pause));
        assert_eq!(
            allowed(Op::SkipEntry),
            [Transferring, Paused, Retrying, Waiting]
        );
        assert_eq!(
            allowed(Op::Cancel).len(),
            10,
            "every state but the 4 terminal ones"
        );
        // Every operation that changes state only runs where that change is allowed.
        for s in allowed(Op::Analyse) {
            assert!(can_go(s, Analysing));
        }
        for s in allowed(Op::Start) {
            assert!(can_go(s, Transferring));
        }
        for s in allowed(Op::Pause) {
            assert!(can_go(s, Paused));
        }
        for s in allowed(Op::Cancel) {
            assert!(can_go(s, Cancelled));
        }
        for s in allowed(Op::DiscardDraft) {
            assert!(can_go(s, Cancelled));
        }
    }
}
