//! Turning dry-run output into a structured "what would change" summary.
//!
//! A dry-run is only useful if the user can see its shape at a glance instead of
//! scrolling raw log lines. This module classifies each output line into a
//! [`Change`] and tallies them into a [`DryRunSummary`] the UI can render as
//! "N new · N updated · N to delete" — and, crucially, fold into the
//! destructive-confirmation prompt ("this will delete 1,243 files. Continue?").
//!
//! Two line formats are understood:
//! - **rsync** with `--itemize-changes` (added automatically for dry-runs).
//! - **rclone** `--dry-run`, whose NOTICE lines say "Skipped <action> as
//!   --dry-run is set".

use crate::Tool;

/// A single change a run would make at the destination.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Change {
    /// A file that does not exist at the destination would be created.
    Added,
    /// An existing destination file would be overwritten/updated.
    Updated,
    /// A destination file/dir would be removed (mirror/delete/move).
    Deleted,
}

/// Running tally of the changes a dry-run would make.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct DryRunSummary {
    pub added: u64,
    pub updated: u64,
    pub deleted: u64,
}

impl DryRunSummary {
    /// Feed one already-sanitized output line, updating the tally.
    pub fn record_line(&mut self, tool: Tool, line: &str) {
        let change = match tool {
            Tool::Rsync => classify_rsync(line),
            Tool::Rclone => classify_rclone(line),
        };
        if let Some(c) = change {
            self.record(c);
        }
    }

    pub fn record(&mut self, change: Change) {
        match change {
            Change::Added => self.added += 1,
            Change::Updated => self.updated += 1,
            Change::Deleted => self.deleted += 1,
        }
    }

    /// Whether anything at all would change.
    pub fn is_empty(&self) -> bool {
        self.added == 0 && self.updated == 0 && self.deleted == 0
    }

    /// A one-line human summary, e.g. `12 new · 3 updated · 5 to delete`.
    pub fn describe(&self) -> String {
        if self.is_empty() {
            return "no changes".to_string();
        }
        let mut parts = Vec::new();
        if self.added > 0 {
            parts.push(format!("{} new", self.added));
        }
        if self.updated > 0 {
            parts.push(format!("{} updated", self.updated));
        }
        if self.deleted > 0 {
            parts.push(format!("{} to delete", self.deleted));
        }
        parts.join(" · ")
    }
}

/// Classify an rsync `--itemize-changes` line.
///
/// The itemize prefix is 11 chars: `YXcstpoguax` — `Y` update type, `X` file
/// type, then attribute flags. A newly created file has all `+`; deletions come
/// as `*deleting <name>`.
fn classify_rsync(line: &str) -> Option<Change> {
    let trimmed = line.trim_start();
    if trimmed.starts_with("*deleting") {
        return Some(Change::Deleted);
    }
    let bytes = trimmed.as_bytes();
    // Need at least the 11-char code plus a following separator.
    if bytes.len() < 12 {
        return None;
    }
    // Only count regular files (X == 'f'); dirs/symlinks are structural noise.
    let update_type = bytes[0];
    if !matches!(update_type, b'>' | b'<' | b'c' | b'h') || bytes[1] != b'f' {
        return None;
    }
    // A brand-new file has all-'+' attributes; anything else is an update.
    let attrs = &trimmed[2..11];
    if attrs == "+++++++++" {
        Some(Change::Added)
    } else {
        Some(Change::Updated)
    }
}

/// Classify an rclone `--dry-run` NOTICE line ("Skipped <action> as --dry-run
/// is set").
fn classify_rclone(line: &str) -> Option<Change> {
    // The message is stable across rclone versions; match on the action verb.
    if !line.contains("as --dry-run is set") {
        return None;
    }
    if line.contains("Skipped delete") || line.contains("Skipped remove") {
        Some(Change::Deleted)
    } else if line.contains("Skipped update") {
        Some(Change::Updated)
    } else if line.contains("Skipped copy") || line.contains("Skipped move") {
        Some(Change::Added)
    } else {
        // e.g. "Skipped set modification time" — not a content change.
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rsync_new_file_is_added() {
        assert_eq!(classify_rsync(">f+++++++++ photo.jpg"), Some(Change::Added));
    }

    #[test]
    fn rsync_changed_file_is_updated() {
        assert_eq!(
            classify_rsync(">f.st...... report.pdf"),
            Some(Change::Updated)
        );
    }

    #[test]
    fn rsync_deletion_is_deleted() {
        assert_eq!(
            classify_rsync("*deleting old/stale.txt"),
            Some(Change::Deleted)
        );
        assert_eq!(
            classify_rsync("*deleting removed_dir/"),
            Some(Change::Deleted)
        );
    }

    #[test]
    fn rsync_directory_and_noise_ignored() {
        assert_eq!(classify_rsync("cd+++++++++ newdir/"), None); // a dir, not a file
        assert_eq!(classify_rsync("sending incremental file list"), None);
        assert_eq!(classify_rsync(""), None);
    }

    #[test]
    fn rclone_actions_map_correctly() {
        assert_eq!(
            classify_rclone("NOTICE: a.txt: Skipped copy as --dry-run is set"),
            Some(Change::Added)
        );
        assert_eq!(
            classify_rclone("NOTICE: a.txt: Skipped update as --dry-run is set (size 1)"),
            Some(Change::Updated)
        );
        assert_eq!(
            classify_rclone("NOTICE: a.txt: Skipped delete as --dry-run is set"),
            Some(Change::Deleted)
        );
        assert_eq!(
            classify_rclone("NOTICE: a.txt: Skipped set modification time as --dry-run is set"),
            None
        );
        assert_eq!(classify_rclone("Transferred: 0 B / 0 B, -, 0 B/s"), None);
    }

    #[test]
    fn summary_tallies_and_describes() {
        let mut s = DryRunSummary::default();
        for line in [
            ">f+++++++++ new1",
            ">f+++++++++ new2",
            ">f.st...... changed1",
            "*deleting gone1",
        ] {
            s.record_line(Tool::Rsync, line);
        }
        assert_eq!(s.added, 2);
        assert_eq!(s.updated, 1);
        assert_eq!(s.deleted, 1);
        assert!(!s.is_empty());
        assert_eq!(s.describe(), "2 new · 1 updated · 1 to delete");
    }

    #[test]
    fn empty_summary_describes_no_changes() {
        assert_eq!(DryRunSummary::default().describe(), "no changes");
        assert!(DryRunSummary::default().is_empty());
    }

    #[test]
    fn rsync_receive_direction_new_file_is_added() {
        // '<' (receiving) is as valid as '>' (sending) for a transfer.
        assert_eq!(
            classify_rsync("<f+++++++++ incoming.bin"),
            Some(Change::Added)
        );
    }

    #[test]
    fn rsync_non_regular_files_are_ignored() {
        // Symlink (L) and device/other types are not counted as file changes.
        assert_eq!(classify_rsync(">L+++++++++ link -> target"), None);
        assert_eq!(classify_rsync("cL+++++++++ newlink"), None);
    }

    #[test]
    fn rsync_short_or_dotprefixed_lines_are_ignored() {
        // Too short to hold the 11-char itemize code + a filename.
        assert_eq!(classify_rsync(">f+++++++++"), None);
        // A no-op itemize line (leading '.') is not a content change.
        assert_eq!(classify_rsync(".f          unchanged.txt"), None);
    }

    #[test]
    fn rclone_move_counts_as_added() {
        assert_eq!(
            classify_rclone("NOTICE: x: Skipped move as --dry-run is set"),
            Some(Change::Added)
        );
    }

    #[test]
    fn rclone_priority_prefixed_lines_still_parse() {
        // journald-style "<5>" priority prefix must not defeat matching.
        assert_eq!(
            classify_rclone("<5>NOTICE: a: Skipped delete as --dry-run is set (size 6)"),
            Some(Change::Deleted)
        );
    }

    #[test]
    fn describe_single_categories() {
        let mut only_del = DryRunSummary::default();
        only_del.record(Change::Deleted);
        only_del.record(Change::Deleted);
        assert_eq!(only_del.describe(), "2 to delete");

        let mut only_new = DryRunSummary::default();
        only_new.record(Change::Added);
        assert_eq!(only_new.describe(), "1 new");
    }

    #[test]
    fn record_line_ignores_wrong_tool_format() {
        // An rclone-style line fed as rsync (and vice-versa) must not miscount.
        let mut s = DryRunSummary::default();
        s.record_line(Tool::Rsync, "NOTICE: x: Skipped copy as --dry-run is set");
        s.record_line(Tool::Rclone, ">f+++++++++ file");
        assert!(s.is_empty(), "cross-tool lines must not be counted");
    }
}
