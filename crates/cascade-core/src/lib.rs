//! `cascade-core` — GTK-free business logic for Cascade.
//!
//! This crate contains everything that can be tested without a display server:
//! command builders, the async process runner, the job state machine, storage,
//! and the security primitives (path validation, destructive-op detection, log
//! sanitization). The GUI crate (`cascade-gui`) is a thin layer on top.
//!
//! Design rule: **no command is ever built as a shell string.** Every external
//! tool is invoked with an explicit argument vector (`Vec<String>`).

pub mod config;
pub mod error;
pub mod job;
pub mod process;
pub mod rclone;
pub mod rsync;
pub mod security;
pub mod storage;

pub use error::{CoreError, Result};

/// Tools Cascade orchestrates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tool {
    Rclone,
    Rsync,
}

impl Tool {
    pub fn binary(self) -> &'static str {
        match self {
            Tool::Rclone => "rclone",
            Tool::Rsync => "rsync",
        }
    }
}
