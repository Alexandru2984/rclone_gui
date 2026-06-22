//! On-disk, per-run log files.
//!
//! Lines handed here are already sanitized by the process runner. The writer
//! tallies error/warning/info counts (for the log filter UI) and stores the
//! file with private (0600) permissions.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use serde::Serialize;

/// Delete `*.log` files in `dir` whose modification time is before `older_than`.
/// Returns how many were removed. Best-effort: I/O errors on individual files
/// are ignored.
pub fn prune_logs(dir: &Path, older_than: SystemTime) -> std::io::Result<usize> {
    let mut removed = 0;
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return Ok(0), // no log dir yet
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("log") {
            continue;
        }
        if let Ok(modified) = entry.metadata().and_then(|m| m.modified()) {
            if modified < older_than && std::fs::remove_file(&path).is_ok() {
                removed += 1;
            }
        }
    }
    Ok(removed)
}

/// Prune `*.log` files older than `days` (from now).
pub fn prune_logs_older_than_days(dir: &Path, days: u64) -> std::io::Result<usize> {
    let cutoff = SystemTime::now()
        .checked_sub(Duration::from_secs(days.saturating_mul(86_400)))
        .unwrap_or(SystemTime::UNIX_EPOCH);
    prune_logs(dir, cutoff)
}

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

/// Severity of a single log line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Error,
    Warning,
    Info,
}

/// Best-effort severity classification from common rsync/rclone phrasing.
pub fn classify(line: &str) -> Level {
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

    #[test]
    fn prune_removes_only_old_logs() {
        let dir = tempfile::tempdir().unwrap();
        LogWriter::create(dir.path(), 1).unwrap();
        LogWriter::create(dir.path(), 2).unwrap();
        std::fs::write(dir.path().join("keep.txt"), b"not a log").unwrap();

        // Cutoff in the future → both .log files are "old" and removed; .txt kept.
        let future = SystemTime::now() + Duration::from_secs(3600);
        assert_eq!(prune_logs(dir.path(), future).unwrap(), 2);
        assert!(dir.path().join("keep.txt").exists());
        assert!(!dir.path().join("run-1.log").exists());

        // Nothing left, and a past cutoff removes nothing.
        LogWriter::create(dir.path(), 3).unwrap();
        let past = SystemTime::now() - Duration::from_secs(3600);
        assert_eq!(prune_logs(dir.path(), past).unwrap(), 0);
        assert!(dir.path().join("run-3.log").exists());
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
