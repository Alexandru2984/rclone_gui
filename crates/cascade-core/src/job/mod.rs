//! Job model: a configured operation plus the live status of its current run.

pub mod spec;
pub mod state;

pub use spec::{JobSpec, OpKind};
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
