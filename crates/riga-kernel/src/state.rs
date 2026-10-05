use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RigaErrorCode {
    InvalidRequest,
    SessionNotFound,
    RunNotFound,
    InvalidStateTransition,
    PersistenceError,
    Internal,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RigaError {
    pub code: RigaErrorCode,
    pub message: String,
    pub retryable: bool,
}

impl RigaError {
    pub fn invalid_transition(message: impl Into<String>) -> Self {
        Self {
            code: RigaErrorCode::InvalidStateTransition,
            message: message.into(),
            retryable: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RunState {
    Created,
    Queued,
    Running,
    WaitingForApproval,
    Paused,
    Resuming,
    Completed,
    Failed,
    Cancelled,
}

impl RunState {
    pub fn transition(self, next: Self) -> Result<Self, RigaError> {
        let valid = matches!(
            (self, next),
            (Self::Created, Self::Queued)
                | (Self::Queued, Self::Running)
                | (Self::Running, Self::WaitingForApproval)
                | (Self::WaitingForApproval, Self::Running)
                | (Self::Running, Self::Paused)
                | (Self::Paused, Self::Resuming)
                | (Self::Resuming, Self::Running)
                | (Self::Running, Self::Completed)
                | (Self::Running, Self::Failed)
                | (Self::Running, Self::Cancelled)
        );
        if valid {
            Ok(next)
        } else {
            Err(RigaError::invalid_transition(format!(
                "cannot transition from {self:?} to {next:?}"
            )))
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Session {
    pub id: String,
    pub title: String,
    pub workspace: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunRecord {
    pub id: String,
    pub session_id: String,
    pub state: RunState,
    pub prompt: String,
}

impl RunRecord {
    pub fn cancel(&mut self) -> Result<(), RigaError> {
        self.state = self.state.transition(RunState::Cancelled)?;
        Ok(())
    }

    pub fn complete(&mut self) -> Result<(), RigaError> {
        self.state = self.state.transition(RunState::Completed)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{RigaErrorCode, RunRecord, RunState};

    #[test]
    fn valid_run_lifecycle_transitions() {
        let mut state = RunState::Created;
        for next in [RunState::Queued, RunState::Running, RunState::Cancelled] {
            state = state.transition(next).expect("valid transition");
        }
        assert_eq!(state, RunState::Cancelled);
    }

    #[test]
    fn cancelled_run_cannot_complete() {
        let mut run = RunRecord {
            id: "run-1".into(),
            session_id: "session-1".into(),
            state: RunState::Running,
            prompt: "cancel me".into(),
        };
        run.cancel().expect("cancellation is valid");
        let error = run.complete().expect_err("cancelled run cannot complete");
        assert_eq!(error.code, RigaErrorCode::InvalidStateTransition);
    }

    #[test]
    fn invalid_transition_is_typed() {
        let error = RunState::Created
            .transition(RunState::Completed)
            .expect_err("created cannot complete directly");
        assert_eq!(error.code, RigaErrorCode::InvalidStateTransition);
    }
}
