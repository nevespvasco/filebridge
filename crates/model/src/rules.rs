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
    /// The runner moving a job through stages 4–6.
    Run,
    /// Startup: picking up a job the app left unfinished.
    Recover,
}

impl Op {
    /// Whether this operation may run on a job in `state`.
    pub fn allowed_in(self, state: JobState) -> bool {
        use JobState::*;
        match self {
            Op::SetOptions | Op::Analyse => matches!(state, Draft | AwaitingApproval | Ready),
            Op::Decide => matches!(state, AwaitingApproval | Ready),
            Op::Start => matches!(state, Ready | Paused),
            Op::Pause | Op::Run => state.is_running(),
            Op::Cancel | Op::Recover => !state.is_terminal(),
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
        assert_eq!(allowed(Op::Decide), [AwaitingApproval, Ready]);
        assert_eq!(allowed(Op::Start), [Ready, Paused]);
        assert_eq!(
            allowed(Op::Pause),
            [Transferring, Retrying, Waiting, Verifying, Integrating]
        );
        assert_eq!(allowed(Op::Run), allowed(Op::Pause));
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
    }
}
