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
        matches!(
            (self, next),
            (Pending, Running)
                | (Pending, Cancelled)
                | (Running, Paused)
                | (Running, Completed)
                | (Running, Failed)
                | (Running, Cancelled)
                | (Paused, Running)
                | (Paused, Cancelled)
        )
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

    #[test]
    fn pending_can_only_run_or_cancel() {
        assert!(Pending.transition(Running).is_ok());
        assert!(Pending.transition(Cancelled).is_ok());
        // Not straight to Paused / Completed / Failed.
        assert!(Pending.transition(Paused).is_err());
        assert!(Pending.transition(Completed).is_err());
        assert!(Pending.transition(Failed).is_err());
    }

    #[test]
    fn paused_cannot_go_directly_to_a_finished_state() {
        assert!(Paused.transition(Running).is_ok());
        assert!(Paused.transition(Cancelled).is_ok());
        assert!(Paused.transition(Completed).is_err());
        assert!(Paused.transition(Failed).is_err());
    }

    #[test]
    fn self_transitions_are_rejected() {
        for s in [Pending, Running, Paused, Completed, Failed, Cancelled] {
            assert!(s.transition(s).is_err(), "{s:?} -> {s:?} should be illegal");
        }
    }

    #[test]
    fn non_terminal_states_report_as_such() {
        for s in [Pending, Running, Paused] {
            assert!(!s.is_terminal());
        }
    }

    #[test]
    fn transition_error_carries_endpoints() {
        match Completed.transition(Running) {
            Err(crate::error::CoreError::IllegalTransition { from, to }) => {
                assert_eq!(from, Completed);
                assert_eq!(to, Running);
            }
            other => panic!("expected IllegalTransition, got {other:?}"),
        }
    }

    #[test]
    fn status_serde_is_lowercase() {
        let json = serde_json::to_string(&Running).unwrap();
        assert_eq!(json, "\"running\"");
        let back: super::JobStatus = serde_json::from_str("\"paused\"").unwrap();
        assert_eq!(back, Paused);
    }
}
