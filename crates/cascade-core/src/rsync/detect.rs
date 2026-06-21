//! rsync binary detection.

use crate::rclone::detect::{detect_named, ToolInfo};

/// Probe for `rsync`. Returns `None` if not installed.
pub fn detect() -> Option<ToolInfo> {
    detect_named("rsync", &["--version"])
}
