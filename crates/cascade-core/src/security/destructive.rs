//! Destructive-operation detection.
//!
//! Every operation is classified into a [`RiskLevel`]. The GUI uses this to
//! decide whether to show a danger style, force a confirmation dialog, and
//! default the action to a dry-run.

/// How dangerous an operation is for existing data.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum RiskLevel {
    /// Cannot remove data (e.g. `copy`, `check`, `size`, `ls`).
    Safe,
    /// Can overwrite, but only adds/updates by default (e.g. `sync` without delete).
    Caution,
    /// Can delete data (e.g. `delete`, `purge`, `sync --delete`, `move`).
    Destructive,
}

impl RiskLevel {
    pub fn requires_confirmation(self) -> bool {
        self == RiskLevel::Destructive
    }
    pub fn recommends_dry_run(self) -> bool {
        self >= RiskLevel::Caution
    }
}

/// Operation kinds across both tools, normalized.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Operation {
    Copy,
    Sync,
    Move,
    /// Two-way sync (rclone `bisync`) — can delete on either side.
    Bisync,
    Check,
    Size,
    Ls,
    Delete,
    Purge,
    Mount,
}

/// Classify an operation, taking into account whether deletion is enabled
/// (relevant for `sync`/rsync where `--delete` is opt-in).
pub fn classify(op: Operation, delete_enabled: bool) -> RiskLevel {
    match op {
        Operation::Check | Operation::Size | Operation::Ls | Operation::Mount => RiskLevel::Safe,
        Operation::Copy => RiskLevel::Caution, // can overwrite
        Operation::Sync => {
            if delete_enabled {
                RiskLevel::Destructive
            } else {
                RiskLevel::Caution
            }
        }
        Operation::Move | Operation::Delete | Operation::Purge | Operation::Bisync => {
            RiskLevel::Destructive
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_only_ops_are_safe() {
        for op in [Operation::Check, Operation::Size, Operation::Ls] {
            assert_eq!(classify(op, false), RiskLevel::Safe);
        }
    }

    #[test]
    fn copy_is_caution_not_destructive() {
        assert_eq!(classify(Operation::Copy, false), RiskLevel::Caution);
        assert!(!classify(Operation::Copy, false).requires_confirmation());
    }

    #[test]
    fn sync_escalates_with_delete() {
        assert_eq!(classify(Operation::Sync, false), RiskLevel::Caution);
        assert_eq!(classify(Operation::Sync, true), RiskLevel::Destructive);
    }

    #[test]
    fn delete_and_purge_are_destructive() {
        assert!(classify(Operation::Delete, false).requires_confirmation());
        assert!(classify(Operation::Purge, false).requires_confirmation());
        assert!(classify(Operation::Move, false).requires_confirmation());
    }

    #[test]
    fn dry_run_recommended_for_caution_and_up() {
        assert!(!classify(Operation::Ls, false).recommends_dry_run());
        assert!(classify(Operation::Copy, false).recommends_dry_run());
        assert!(classify(Operation::Sync, true).recommends_dry_run());
    }

    #[test]
    fn bisync_is_always_destructive() {
        // The delete flag is irrelevant for bisync — it can delete on both sides.
        assert_eq!(classify(Operation::Bisync, false), RiskLevel::Destructive);
        assert_eq!(classify(Operation::Bisync, true), RiskLevel::Destructive);
        assert!(classify(Operation::Bisync, false).requires_confirmation());
    }

    #[test]
    fn mount_is_safe() {
        assert_eq!(classify(Operation::Mount, false), RiskLevel::Safe);
        assert!(!classify(Operation::Mount, false).requires_confirmation());
    }

    #[test]
    fn risk_levels_are_ordered() {
        assert!(RiskLevel::Safe < RiskLevel::Caution);
        assert!(RiskLevel::Caution < RiskLevel::Destructive);
        // Only Destructive needs confirmation.
        assert!(!RiskLevel::Safe.requires_confirmation());
        assert!(!RiskLevel::Caution.requires_confirmation());
        assert!(RiskLevel::Destructive.requires_confirmation());
    }

    #[test]
    fn copy_delete_flag_does_not_change_copy_classification() {
        // Copy is Caution regardless of the delete_enabled hint (which only
        // affects Sync); JobSpec handles Copy+delete escalation separately.
        assert_eq!(classify(Operation::Copy, true), RiskLevel::Caution);
    }
}
