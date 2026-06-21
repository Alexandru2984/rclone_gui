//! On-disk, per-run log files.
//!
//! Lines handed here are already sanitized by the process runner. The writer
//! tallies error/warning/info counts (for the log filter UI) and stores the
//! file with private (0600) permissions.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::Serialize;

/// Running tally of log severities for a single run.
#[derive(Debug, Default, Clone, Serialize)]
pub struct LevelCounts {
    pub errors: u64,
    pub warnings: u64,
    pub info: u64,
}

/// Appends sanitized lines to a per-run file and counts severities.
pub struct LogWriter {
    file: File,
    path: PathBuf,
    counts: LevelCounts,
}

impl LogWriter {
    /// Create `dir/run-<run_id>.log` (private perms) for appending.
    pub fn create(dir: &Path, run_id: i64) -> std::io::Result<Self> {
        std::fs::create_dir_all(dir)?;
        let path = dir.join(format!("run-{run_id}.log"));
        let file = OpenOptions::new().create(true).append(true).open(&path)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
        }
        Ok(Self {
            file,
            path,
            counts: LevelCounts::default(),
        })
    }

    /// Classify and append a single line.
    pub fn write_line(&mut self, line: &str) -> std::io::Result<()> {
        match classify(line) {
            Level::Error => self.counts.errors += 1,
            Level::Warning => self.counts.warnings += 1,
            Level::Info => self.counts.info += 1,
        }
        writeln!(self.file, "{line}")
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn counts(&self) -> &LevelCounts {
        &self.counts
    }

    /// Counts serialized as JSON for the `run_logs.level_counts_json` column.
    pub fn counts_json(&self) -> String {
        serde_json::to_string(&self.counts).unwrap_or_else(|_| "{}".into())
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Level {
    Error,
    Warning,
    Info,
}

/// Best-effort severity classification from common rsync/rclone phrasing.
fn classify(line: &str) -> Level {
    let l = line.to_ascii_lowercase();
    if l.contains("error") || l.contains("failed") || l.contains("[error]") {
        Level::Error
    } else if l.contains("warning") || l.contains("warn") {
        Level::Warning
    } else {
        Level::Info
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_file_and_counts_levels() {
        let dir = tempfile::tempdir().unwrap();
        let mut w = LogWriter::create(dir.path(), 42).unwrap();
        w.write_line("starting copy").unwrap();
        w.write_line("WARNING: skipping symlink").unwrap();
        w.write_line("ERROR: permission denied").unwrap();
        w.write_line("transfer failed for file x").unwrap();

        assert_eq!(w.counts().errors, 2);
        assert_eq!(w.counts().warnings, 1);
        assert_eq!(w.counts().info, 1);

        let contents = std::fs::read_to_string(w.path()).unwrap();
        assert!(contents.contains("starting copy"));
        assert_eq!(contents.lines().count(), 4);
    }

    #[cfg(unix)]
    #[test]
    fn file_is_private() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let w = LogWriter::create(dir.path(), 1).unwrap();
        let mode = std::fs::metadata(w.path()).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }
}
