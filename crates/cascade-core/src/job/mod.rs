//! Job model: a configured operation plus the live status of its current run.

pub mod queue;
pub mod spec;
pub mod state;

pub use queue::Queue;
pub use spec::{AdvancedOptions, JobSpec, OpKind};
pub use state::JobStatus;

use serde::{Deserialize, Serialize};

use crate::Tool;

/// Live progress snapshot, updated from parsed process output.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Progress {
    pub percent: Option<f32>,
    pub bytes_transferred: u64,
    pub files_done: u64,
    pub speed_bps: Option<u64>,
    /// Estimated seconds remaining, when known.
    pub eta_secs: Option<u64>,
}

/// A configured job. The generated argv is kept for preview and audit.
#[derive(Debug, Clone)]
pub struct Job {
    pub id: Option<i64>,
    pub name: String,
    pub tool: Tool,
    pub source: String,
    pub destination: String,
    pub dry_run: bool,
    /// Generated argv (display preview is derived from this).
    pub argv: Vec<String>,
    pub status: JobStatus,
    pub progress: Progress,
}

impl Job {
    /// Create a pending job from an already-built argv.
    pub fn new(
        name: impl Into<String>,
        tool: Tool,
        source: impl Into<String>,
        destination: impl Into<String>,
        dry_run: bool,
        argv: Vec<String>,
    ) -> Self {
        Self {
            id: None,
            name: name.into(),
            tool,
            source: source.into(),
            destination: destination.into(),
            dry_run,
            argv,
            status: JobStatus::Pending,
            progress: Progress::default(),
        }
    }

    /// Attempt a status transition, updating in place on success.
    pub fn set_status(&mut self, next: JobStatus) -> crate::Result<()> {
        self.status = self.status.transition(next)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn job() -> Job {
        Job::new(
            "nightly",
            Tool::Rsync,
            "/src/",
            "/dst/",
            false,
            vec!["-a".into(), "/src/".into(), "/dst/".into()],
        )
    }

    #[test]
    fn new_job_starts_pending_with_default_progress() {
        let j = job();
        assert_eq!(j.status, JobStatus::Pending);
        assert_eq!(j.id, None);
        assert_eq!(j.name, "nightly");
        assert_eq!(j.tool, Tool::Rsync);
        assert_eq!(j.argv.len(), 3);
        assert_eq!(j.progress.percent, None);
        assert_eq!(j.progress.bytes_transferred, 0);
    }

    #[test]
    fn set_status_applies_legal_transitions() {
        let mut j = job();
        j.set_status(JobStatus::Running).unwrap();
        assert_eq!(j.status, JobStatus::Running);
        j.set_status(JobStatus::Completed).unwrap();
        assert_eq!(j.status, JobStatus::Completed);
    }

    #[test]
    fn set_status_rejects_illegal_transitions_without_mutating() {
        let mut j = job();
        // Pending -> Completed is illegal; the error must leave status unchanged.
        assert!(j.set_status(JobStatus::Completed).is_err());
        assert_eq!(j.status, JobStatus::Pending);
    }

    #[test]
    fn progress_default_is_empty() {
        let p = Progress::default();
        assert_eq!(p.percent, None);
        assert_eq!(p.files_done, 0);
        assert_eq!(p.eta_secs, None);
    }
}
