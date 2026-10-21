//! Job kinds, states and the transitions between them (docs/02, "Job state machine").

use serde::{Deserialize, Serialize};

use crate::{CoreError, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum JobKind {
    Transfer,
    /// "Repair destination" (Phase 2).
    Repair,
}

impl JobKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Transfer => "transfer",
            Self::Repair => "repair",
        }
    }

    pub fn parse(s: &str) -> Result<Self> {
        match s {
            "transfer" => Ok(Self::Transfer),
            "repair" => Ok(Self::Repair),
            other => Err(CoreError::Invalid(format!("job kind {other:?}"))),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum JobState {
    Draft,
    Analysing,
    AwaitingApproval,
    Ready,
    Transferring,
    Paused,
    Retrying,
    Verifying,
    Integrating,
    Completed,
    CompletedWithWarnings,
    Cancelled,
    Failed,
}

impl JobState {
    pub const ALL: [JobState; 13] = [
        Self::Draft,
        Self::Analysing,
        Self::AwaitingApproval,
        Self::Ready,
        Self::Transferring,
        Self::Paused,
        Self::Retrying,
        Self::Verifying,
        Self::Integrating,
        Self::Completed,
        Self::CompletedWithWarnings,
        Self::Cancelled,
        Self::Failed,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Draft => "draft",
            Self::Analysing => "analysing",
            Self::AwaitingApproval => "awaiting_approval",
            Self::Ready => "ready",
            Self::Transferring => "transferring",
            Self::Paused => "paused",
            Self::Retrying => "retrying",
            Self::Verifying => "verifying",
            Self::Integrating => "integrating",
            Self::Completed => "completed",
            Self::CompletedWithWarnings => "completed_with_warnings",
            Self::Cancelled => "cancelled",
            Self::Failed => "failed",
        }
    }

    pub fn parse(s: &str) -> Result<Self> {
        Self::ALL
            .into_iter()
            .find(|st| st.as_str() == s)
            .ok_or_else(|| CoreError::Invalid(format!("job state {s:?}")))
    }

    /// No further transitions; the job is never resumed.
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Completed | Self::CompletedWithWarnings | Self::Cancelled | Self::Failed
        )
    }

    /// States in which a runner task works on the job; these resume on startup.
    pub fn is_running(self) -> bool {
        matches!(
            self,
            Self::Transferring | Self::Retrying | Self::Verifying | Self::Integrating
        )
    }

    /// Whether `self → next` is allowed.
    pub fn can_go_to(self, next: JobState) -> bool {
        use JobState::*;
        if self.is_terminal() {
            return false;
        }
        if matches!(next, Cancelled | Failed) {
            return true;
        }
        match self {
            Draft => matches!(next, Analysing),
            Analysing => matches!(next, AwaitingApproval | Ready | Draft),
            AwaitingApproval => matches!(next, Ready | Analysing),
            Ready => matches!(next, Transferring | Analysing | AwaitingApproval),
            Transferring => matches!(next, Paused | Retrying | Verifying | Integrating),
            Paused => matches!(next, Transferring),
            Retrying => matches!(next, Transferring | Verifying | Integrating | Paused),
            Verifying => matches!(next, Transferring | Integrating | Retrying | Paused),
            Integrating => matches!(next, Completed | CompletedWithWarnings | Retrying | Paused),
            Completed | CompletedWithWarnings | Cancelled | Failed => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_round_trip() {
        for st in JobState::ALL {
            assert_eq!(JobState::parse(st.as_str()).unwrap(), st);
        }
        assert!(JobState::parse("nope").is_err());
        assert_eq!(JobKind::parse("transfer").unwrap(), JobKind::Transfer);
    }

    #[test]
    fn transitions() {
        use JobState::*;
        assert!(Draft.can_go_to(Analysing));
        assert!(!Draft.can_go_to(Transferring));
        assert!(Transferring.can_go_to(Retrying));
        assert!(Retrying.can_go_to(Transferring));
        assert!(Verifying.can_go_to(Transferring), "mismatches resend");
        assert!(Integrating.can_go_to(CompletedWithWarnings));
        assert!(Paused.can_go_to(Cancelled));
        assert!(!Completed.can_go_to(Cancelled), "terminal is final");
        assert!(!Failed.can_go_to(Transferring));
    }
}
