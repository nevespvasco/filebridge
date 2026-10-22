//! The Job lifecycle (docs/02, "Job state machine"): enforces which operation may run in which
//! state and which state may follow which (the rules themselves are pure data, in
//! `filebridge_model::rules`), and records each change.
//!
//! Every operation on a job starts with [`Engine::begin`]. It locks the job against every other
//! operation on it, so checking the state and acting on it can't be split by a concurrent
//! request (a decision arriving while Send plans the destination paths). Every state change goes
//! through [`JobLock::go`], which checks the transition table and writes the new state, the
//! finished stage's outcome (warnings, verify rounds) and the `state` event in one store
//! transaction before anyone is told.

use std::collections::HashMap;
use std::sync::Arc;

use filebridge_store::StateChange;
use tokio::sync::OwnedMutexGuard;

use filebridge_model::{Op, can_go};

use crate::{CoreError, Engine, Event, JobState, Result};

/// What a stage produced, saved with the state change that ends it.
#[derive(Debug, Clone, Default)]
pub(crate) struct Outcome {
    pub warnings: Vec<String>,
    /// A verify round found differences and sends those files again.
    pub verify_round: bool,
}

/// One lock per job id.
#[derive(Default)]
pub(crate) struct JobLocks(std::sync::Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>);

impl JobLocks {
    async fn lock(&self, job_id: &str) -> OwnedMutexGuard<()> {
        let lock = self
            .0
            .lock()
            .expect("job locks poisoned")
            .entry(job_id.to_owned())
            .or_default()
            .clone();
        lock.lock_owned().await
    }
}

/// A job locked for one operation. Dropping it lets the next operation in.
pub(crate) struct JobLock<'a> {
    engine: &'a Engine,
    id: String,
    state: JobState,
    warnings: Vec<String>,
    verify_rounds: u32,
    _guard: OwnedMutexGuard<()>,
}

impl Engine {
    /// Locks `job_id` and checks that `op` may run in its current state.
    pub(crate) async fn begin(&self, job_id: &str, op: Op) -> Result<JobLock<'_>> {
        let guard = self.locks.lock(job_id).await;
        let row = self.store.get_job(job_id).await?;
        let state = JobState::parse(&row.state)?;
        if !op.allowed_in(state) {
            return Err(self.refusal(job_id, op, state).await);
        }
        Ok(JobLock {
            engine: self,
            id: row.id,
            state,
            warnings: row.warnings,
            verify_rounds: row.verify_rounds,
            _guard: guard,
        })
    }

    async fn refusal(&self, job_id: &str, op: Op, state: JobState) -> CoreError {
        let invalid = |what: &str| {
            CoreError::Invalid(format!(
                "{what} can't change while the job is {}",
                state.as_str()
            ))
        };
        let transition = |to| CoreError::InvalidTransition { from: state, to };
        match op {
            Op::SetOptions => invalid("options"),
            Op::Decide => invalid("decisions"),
            Op::Analyse => transition(JobState::Analysing),
            Op::Start if state == JobState::AwaitingApproval => {
                match self.store.pending_issue_count(job_id).await {
                    Ok(n) => CoreError::PendingDecisions(n),
                    Err(e) => e.into(),
                }
            }
            Op::Start => transition(JobState::Transferring),
            Op::Pause => transition(JobState::Paused),
            Op::Cancel => transition(JobState::Cancelled),
            Op::Run | Op::Recover => {
                CoreError::Invalid(format!("a job in state {} can't run", state.as_str()))
            }
        }
    }
}

impl JobLock<'_> {
    pub(crate) fn id(&self) -> &str {
        &self.id
    }

    pub(crate) fn state(&self) -> JobState {
        self.state
    }

    pub(crate) fn verify_rounds(&self) -> u32 {
        self.verify_rounds
    }

    /// Moves the job to `to`.
    pub(crate) async fn go(&mut self, to: JobState, error: Option<String>) -> Result<()> {
        self.go_with(to, error, Outcome::default()).await
    }

    /// Ends the last stage: Completed, or CompletedWithWarnings if any stage (before a restart
    /// too) or `outcome` left a warning.
    pub(crate) async fn complete(&mut self, outcome: Outcome) -> Result<()> {
        let all: Vec<_> = self
            .warnings
            .iter()
            .chain(&outcome.warnings)
            .cloned()
            .collect();
        if all.is_empty() {
            self.go_with(JobState::Completed, None, outcome).await
        } else {
            let error = Some(all.join("; "));
            self.go_with(JobState::CompletedWithWarnings, error, outcome)
                .await
        }
    }

    /// Moves the job to `to`, saving what the stage that ends produced.
    pub(crate) async fn go_with(
        &mut self,
        to: JobState,
        error: Option<String>,
        outcome: Outcome,
    ) -> Result<()> {
        let from = self.state;
        if from == to && outcome.warnings.is_empty() && !outcome.verify_round {
            return Ok(());
        }
        if from != to && !can_go(from, to) {
            return Err(CoreError::InvalidTransition { from, to });
        }
        let engine = self.engine;
        engine
            .store
            .change_job_state(
                &self.id,
                StateChange {
                    from: from.as_str().into(),
                    to: to.as_str().into(),
                    error: error.clone(),
                    finished: to.is_terminal(),
                    warnings: outcome.warnings.clone(),
                    verify_round: outcome.verify_round,
                },
            )
            .await?;
        self.state = to;
        self.warnings.extend(outcome.warnings);
        self.verify_rounds += u32::from(outcome.verify_round);
        if from == to {
            return Ok(());
        }
        engine.emit(Event::JobStateChanged {
            job_id: self.id.clone(),
            from,
            to,
            error: error.clone(),
        });
        if to.is_terminal() {
            engine
                .log(
                    Some(&self.id),
                    "finished",
                    serde_json::json!({
                        "state": to.as_str(),
                        "warnings": self.warnings.len(),
                        "error": error,
                    }),
                )
                .await?;
            engine.emit(Event::JobFinished {
                job_id: self.id.clone(),
                state: to,
                error,
            });
        }
        Ok(())
    }
}
