//! Crate-wide error type.

use thiserror::Error;

pub type Result<T> = std::result::Result<T, CoreError>;

#[derive(Debug, Error)]
pub enum CoreError {
    #[error("invalid path: {0}")]
    InvalidPath(String),

    #[error("refused dangerous path: {0}")]
    DangerousPath(String),

    #[error("invalid command: {0}")]
    InvalidCommand(String),

    #[error("illegal job state transition: {from:?} -> {to:?}")]
    IllegalTransition {
        from: crate::job::state::JobStatus,
        to: crate::job::state::JobStatus,
    },

    #[error("tool not found: {0}")]
    ToolNotFound(String),

    #[error("process error: {0}")]
    Process(String),

    #[error(transparent)]
    Io(#[from] std::io::Error),

    #[error(transparent)]
    Db(#[from] rusqlite::Error),

    #[error(transparent)]
    Json(#[from] serde_json::Error),
}
