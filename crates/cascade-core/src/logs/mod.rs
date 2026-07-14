//! On-disk, per-run log files.
//!
//! Lines handed here are already sanitized by the process runner. The writer
//! tallies error/warning/info counts (for the log filter UI) and stores the
//! file with private (0600) permissions.

use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use serde::Serialize;

use crate::security::sanitize::StreamRedactor;

const MAX_EXISTING_LOG_LINE_BYTES: usize = 64 * 1024;

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

/// Read only the newest bounded portion of a regular log file.
///
/// This is intended for history/details views: a very large or replaced log
/// can never force the UI to allocate the whole file. Symlinks are refused.
pub fn read_log_tail(
    path: &Path,
    max_bytes: usize,
    max_lines: usize,
) -> std::io::Result<Vec<String>> {
    if max_bytes == 0 || max_lines == 0 {
        return Ok(Vec::new());
    }
    let metadata = std::fs::symlink_metadata(path)?;
    if !metadata.file_type().is_file() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "log path must be a regular file, not a symlink",
        ));
    }
    let mut file = File::open(path)?;
    let start = metadata.len().saturating_sub(max_bytes as u64);
    file.seek(SeekFrom::Start(start))?;
    let mut bytes = Vec::with_capacity((metadata.len() - start) as usize);
    file.take(max_bytes as u64).read_to_end(&mut bytes)?;

    let mut omitted = start > 0;
    if start > 0 {
        if let Some(newline) = bytes.iter().position(|byte| *byte == b'\n') {
            bytes.drain(..=newline);
        } else {
            bytes.clear();
        }
    }
    let text = String::from_utf8_lossy(&bytes);
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    if lines.len() > max_lines {
        let drop_count = lines.len() - max_lines;
        lines.drain(..drop_count);
        omitted = true;
    }
    if omitted {
        lines.insert(0, "… older log output omitted …".to_string());
    }
    Ok(lines)
}

/// Re-sanitize historical log files created by older Cascade versions.
///
/// Files are read in fixed chunks, lines are capped, PEM state is tracked
/// across lines, and replacements are written atomically with mode 0600.
/// Symlinks and non-regular files are ignored.
pub fn sanitize_existing_logs(dir: &Path) -> std::io::Result<usize> {
    match std::fs::symlink_metadata(dir) {
        Ok(metadata) if metadata.file_type().is_dir() => {}
        Ok(_) => {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "log directory must be a real directory, not a symlink",
            ));
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(error) => return Err(error),
    }
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(error) => return Err(error),
    };
    let mut rewritten = 0;
    for (index, entry) in entries.flatten().enumerate() {
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("log") {
            continue;
        }
        let metadata = match std::fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.file_type().is_file() => metadata,
            _ => continue,
        };

        let temp_path = dir.join(format!(".cascade-sanitize-{}-{index}", std::process::id()));
        let input = File::open(&path)?;
        let mut output_options = OpenOptions::new();
        output_options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            output_options.mode(0o600);
        }
        let mut output = output_options.open(&temp_path)?;
        let result = sanitize_log_stream(input, &mut output);
        let changed = match result {
            Ok(changed) => changed,
            Err(error) => {
                let _ = std::fs::remove_file(&temp_path);
                return Err(error);
            }
        };
        output.sync_all()?;

        if changed {
            std::fs::rename(&temp_path, &path)?;
            rewritten += 1;
        } else {
            std::fs::remove_file(&temp_path)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if metadata.permissions().mode() & 0o777 != 0o600 {
                    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
                }
            }
        }
    }
    Ok(rewritten)
}

fn sanitize_log_stream(mut input: File, output: &mut File) -> std::io::Result<bool> {
    let mut redactor = StreamRedactor::new();
    let mut chunk = [0_u8; 8192];
    let mut line = Vec::with_capacity(256);
    let mut truncated = false;
    let mut changed = false;
    loop {
        let count = input.read(&mut chunk)?;
        if count == 0 {
            break;
        }
        for &byte in &chunk[..count] {
            if byte == b'\n' {
                changed |= write_sanitized_line(output, &mut redactor, &line, truncated)?;
                line.clear();
                truncated = false;
            } else if line.len() < MAX_EXISTING_LOG_LINE_BYTES {
                line.push(byte);
            } else {
                truncated = true;
            }
        }
    }
    if !line.is_empty() || truncated {
        changed |= write_sanitized_line(output, &mut redactor, &line, truncated)?;
    }
    Ok(changed)
}

fn write_sanitized_line(
    output: &mut File,
    redactor: &mut StreamRedactor,
    bytes: &[u8],
    truncated: bool,
) -> std::io::Result<bool> {
    let original = String::from_utf8_lossy(bytes);
    let mut line = original.to_string();
    if truncated {
        line.push_str(" …[truncated]");
    }
    match redactor.redact_line(&line) {
        Some(safe) => {
            writeln!(output, "{safe}")?;
            Ok(truncated || safe != original)
        }
        None => Ok(true),
    }
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
    redactor: StreamRedactor,
}

impl LogWriter {
    /// Create `dir/run-<run_id>.log` (private perms) for appending.
    pub fn create(dir: &Path, run_id: i64) -> std::io::Result<Self> {
        std::fs::create_dir_all(dir)?;
        let metadata = std::fs::symlink_metadata(dir)?;
        if !metadata.file_type().is_dir() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "log directory must be a real directory, not a symlink",
            ));
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
        }
        let path = dir.join(format!("run-{run_id}.log"));
        let mut options = OpenOptions::new();
        options.create_new(true).append(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let file = options.open(&path)?;
        Ok(Self {
            file,
            path,
            counts: LevelCounts::default(),
            redactor: StreamRedactor::new(),
        })
    }

    /// Classify and append a single line.
    pub fn write_line(&mut self, line: &str) -> std::io::Result<()> {
        let Some(line) = self.redactor.redact_line(line) else {
            return Ok(());
        };
        match classify(&line) {
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

    #[test]
    fn counts_json_serializes_the_tally() {
        let dir = tempfile::tempdir().unwrap();
        let mut w = LogWriter::create(dir.path(), 7).unwrap();
        w.write_line("ERROR: boom").unwrap();
        w.write_line("plain info").unwrap();
        let json = w.counts_json();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["errors"], 1);
        assert_eq!(parsed["info"], 1);
        assert_eq!(parsed["warnings"], 0);
    }

    #[test]
    fn prune_by_days_keeps_recent_and_tolerates_missing_dir() {
        // A missing directory is not an error, just zero removed.
        let missing = std::path::Path::new("/nonexistent/cascade/logs/xyz");
        assert_eq!(prune_logs_older_than_days(missing, 30).unwrap(), 0);

        // Freshly created logs are newer than the cutoff, so 30-day pruning
        // keeps them.
        let dir = tempfile::tempdir().unwrap();
        LogWriter::create(dir.path(), 1).unwrap();
        assert_eq!(prune_logs_older_than_days(dir.path(), 30).unwrap(), 0);
        assert!(dir.path().join("run-1.log").exists());
    }

    #[test]
    fn classify_recognizes_common_phrasings() {
        assert_eq!(classify("some ERROR happened"), Level::Error);
        assert_eq!(classify("operation failed"), Level::Error);
        assert_eq!(classify("a warning: skipping"), Level::Warning);
        assert_eq!(classify("just some info"), Level::Info);
    }

    #[test]
    fn writer_defensively_redacts_multiline_private_keys() {
        let dir = tempfile::tempdir().unwrap();
        let mut writer = LogWriter::create(dir.path(), 9).unwrap();
        writer
            .write_line("-----BEGIN OPENSSH PRIVATE KEY-----")
            .unwrap();
        writer.write_line("SUPER-SECRET-BODY").unwrap();
        writer
            .write_line("-----END OPENSSH PRIVATE KEY-----")
            .unwrap();
        writer.write_line("safe tail").unwrap();
        let contents = std::fs::read_to_string(writer.path()).unwrap();
        assert!(!contents.contains("SUPER-SECRET-BODY"));
        assert!(!contents.contains("PRIVATE KEY"));
        assert!(contents.contains("safe tail"));
    }

    #[test]
    fn historical_logs_are_resanitized() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("run-1.log");
        std::fs::write(
            &path,
            "safe head\n-----BEGIN RSA PRIVATE KEY-----\nOLD-SECRET-BODY\n-----END RSA PRIVATE KEY-----\n--s3-secret-access-key=aws-secret\nsafe tail\n",
        )
        .unwrap();
        assert_eq!(sanitize_existing_logs(dir.path()).unwrap(), 1);
        let contents = std::fs::read_to_string(path).unwrap();
        assert!(!contents.contains("OLD-SECRET-BODY"));
        assert!(!contents.contains("aws-secret"));
        assert!(contents.contains("safe head"));
        assert!(contents.contains("safe tail"));
    }

    #[test]
    fn writer_never_reopens_an_existing_log() {
        let dir = tempfile::tempdir().unwrap();
        LogWriter::create(dir.path(), 11).unwrap();
        assert!(LogWriter::create(dir.path(), 11).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn writer_refuses_a_preplanted_log_symlink() {
        use std::os::unix::fs::symlink;

        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("target");
        std::fs::write(&target, "do not touch").unwrap();
        symlink(&target, dir.path().join("run-12.log")).unwrap();
        assert!(LogWriter::create(dir.path(), 12).is_err());
        assert_eq!(std::fs::read_to_string(target).unwrap(), "do not touch");
    }

    #[test]
    fn log_tail_is_bounded_by_bytes_and_lines() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("run-20.log");
        let content = (0..100)
            .map(|index| format!("line-{index:03}-xxxxxxxxxxxxxxxx\n"))
            .collect::<String>();
        std::fs::write(&path, content).unwrap();

        let lines = read_log_tail(&path, 400, 5).unwrap();
        assert_eq!(lines.len(), 6);
        assert!(lines[0].contains("omitted"));
        assert!(lines[1].starts_with("line-095"));
        assert!(lines[5].starts_with("line-099"));
    }

    #[cfg(unix)]
    #[test]
    fn log_tail_refuses_symlinks() {
        use std::os::unix::fs::symlink;

        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("target.log");
        let link = dir.path().join("linked.log");
        std::fs::write(&target, "large/untrusted").unwrap();
        symlink(target, &link).unwrap();
        assert!(read_log_tail(&link, 1024, 10).is_err());
    }
}
