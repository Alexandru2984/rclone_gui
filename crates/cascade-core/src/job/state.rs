//! Job state machine.
//!
//! Illegal transitions are rejected at the type/logic level so a finished run
//! can never silently resume and, say, re-run a destructive delete.

use serde::{Deserialize, Serialize};

use crate::error::{CoreError, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum JobStatus {
    Pending,
    Running,
    Paused,
    Completed,
    Failed,
    Cancelled,
}

impl JobStatus {
    /// Terminal states cannot transition any further.
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            JobStatus::Completed | JobStatus::Failed | JobStatus::Cancelled
        )
    }

    /// Whether `self -> next` is a legal transition.
    pub fn can_transition_to(self, next: JobStatus) -> bool {
        use JobStatus::*;
        match (self, next) {
            (Pending, Running) | (Pending, Cancelled) => true,
            (Running, Paused) | (Running, Completed) | (Running, Failed) | (Running, Cancelled) => {
                true
            }
            (Paused, Running) | (Paused, Cancelled) => true,
            _ => false,
        }
    }

    /// Apply a transition, or return an error describing the illegal move.
    pub fn transition(self, next: JobStatus) -> Result<JobStatus> {
        if self.can_transition_to(next) {
            Ok(next)
        } else {
            Err(CoreError::IllegalTransition {
                from: self,
                to: next,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::JobStatus::*;

    #[test]
    fn happy_path() {
        assert_eq!(Pending.transition(Running).unwrap(), Running);
        assert_eq!(Running.transition(Completed).unwrap(), Completed);
    }

    #[test]
    fn pause_and_resume() {
        let s = Running.transition(Paused).unwrap();
        assert_eq!(s.transition(Running).unwrap(), Running);
    }

    #[test]
    fn cannot_resume_completed() {
        assert!(Completed.transition(Running).is_err());
        assert!(Completed.is_terminal());
    }

    #[test]
    fn cannot_skip_to_completed_from_pending() {
        assert!(Pending.transition(Completed).is_err());
    }

    #[test]
    fn terminal_states_are_dead_ends() {
        for s in [Completed, Failed, Cancelled] {
            assert!(s.is_terminal());
            for n in [Running, Paused, Pending] {
                assert!(s.transition(n).is_err());
            }
        }
    }
}
