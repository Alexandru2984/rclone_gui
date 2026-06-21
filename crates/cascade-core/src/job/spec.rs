//! A tool-agnostic job specification.
//!
//! `JobSpec` is the bridge between the UI's intent and the concrete argv passed
//! to rclone/rsync. It also exposes the operation's [`RiskLevel`] so the UI can
//! gate destructive runs. This type is fully unit-testable without a display.

use serde::{Deserialize, Serialize};

use crate::error::Result;
use crate::rclone::command::{self as rclone_cmd, RcloneOp, RcloneOptions};
use crate::rsync::command::{build_args as rsync_args, RsyncOptions};
use crate::security::destructive::{classify, Operation, RiskLevel};
use crate::Tool;

/// The high-level operation, independent of which tool runs it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OpKind {
    /// Add/update files at the destination; never deletes.
    Copy,
    /// Make the destination identical to the source (a mirror) — deletes extras.
    Sync,
    /// Move files (removes them from the source).
    Move,
}

impl OpKind {
    fn as_operation(self) -> Operation {
        match self {
            OpKind::Copy => Operation::Copy,
            OpKind::Sync => Operation::Sync,
            OpKind::Move => Operation::Move,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            OpKind::Copy => "Copy",
            OpKind::Sync => "Sync (mirror)",
            OpKind::Move => "Move",
        }
    }
}

/// A fully-specified job ready to be built into an argv and run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobSpec {
    pub name: String,
    pub tool: Tool,
    pub op: OpKind,
    pub source: String,
    pub destination: String,
    pub dry_run: bool,
    /// For `Copy`: opt-in deletion of dest-only files. (`Sync`/`Move` imply it.)
    pub delete: bool,
}

impl JobSpec {
    /// Whether deletion of destination files actually happens for this spec.
    /// `Sync` mirrors (both tools), so it always deletes extras at the dest.
    pub fn delete_effective(&self) -> bool {
        match self.op {
            OpKind::Sync => true,
            OpKind::Move => true,
            OpKind::Copy => self.delete,
        }
    }

    /// The risk level the UI uses to decide on confirmation + dry-run defaults.
    pub fn risk(&self) -> RiskLevel {
        classify(self.op.as_operation(), self.delete_effective())
    }

    /// The binary that will be invoked.
    pub fn binary(&self) -> &'static str {
        self.tool.binary()
    }

    /// Build the concrete argv. Never produces a shell string.
    pub fn build_argv(&self) -> Result<Vec<String>> {
        match self.tool {
            Tool::Rclone => {
                let op = match self.op {
                    OpKind::Copy => RcloneOp::Copy,
                    OpKind::Sync => RcloneOp::Sync,
                    OpKind::Move => RcloneOp::Move,
                };
                let opts = RcloneOptions { dry_run: self.dry_run, ..Default::default() };
                rclone_cmd::build_args(op, &self.source, Some(&self.destination), &opts)
            }
            Tool::Rsync => {
                let mut opts = RsyncOptions {
                    dry_run: self.dry_run,
                    delete: self.delete_effective(),
                    ..Default::default()
                };
                // rsync has no `move`; emulate it with --remove-source-files.
                if self.op == OpKind::Move {
                    opts.delete = false; // moving is not mirroring
                    opts.extra_flags.push("--remove-source-files".into());
                }
                rsync_args(&self.source, &self.destination, &opts)
            }
        }
    }

    /// A copy-pasteable preview of the command (display only).
    pub fn preview(&self) -> Result<String> {
        let argv = self.build_argv()?;
        Ok(rclone_cmd::preview(self.binary(), &argv))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(tool: Tool, op: OpKind) -> JobSpec {
        JobSpec {
            name: "t".into(),
            tool,
            op,
            source: "/src/".into(),
            destination: "/dst/".into(),
            dry_run: false,
            delete: false,
        }
    }

    #[test]
    fn rclone_copy_builds_copy_argv() {
        let argv = spec(Tool::Rclone, OpKind::Copy).build_argv().unwrap();
        assert_eq!(argv[0], "copy");
        assert_eq!(&argv[1..3], &["/src/".to_string(), "/dst/".to_string()]);
    }

    #[test]
    fn rclone_sync_is_destructive() {
        assert_eq!(spec(Tool::Rclone, OpKind::Sync).risk(), RiskLevel::Destructive);
    }

    #[test]
    fn rclone_copy_is_caution() {
        assert_eq!(spec(Tool::Rclone, OpKind::Copy).risk(), RiskLevel::Caution);
    }

    #[test]
    fn rsync_sync_sets_delete_and_is_destructive() {
        let s = spec(Tool::Rsync, OpKind::Sync);
        assert!(s.build_argv().unwrap().contains(&"--delete".to_string()));
        assert_eq!(s.risk(), RiskLevel::Destructive);
    }

    #[test]
    fn rsync_copy_has_no_delete() {
        let s = spec(Tool::Rsync, OpKind::Copy);
        assert!(!s.build_argv().unwrap().contains(&"--delete".to_string()));
    }

    #[test]
    fn rsync_move_uses_remove_source_files() {
        let s = spec(Tool::Rsync, OpKind::Move);
        let argv = s.build_argv().unwrap();
        assert!(argv.contains(&"--remove-source-files".to_string()));
        assert!(!argv.contains(&"--delete".to_string()));
        assert_eq!(s.risk(), RiskLevel::Destructive);
    }

    #[test]
    fn dry_run_propagates_to_both_tools() {
        let mut r = spec(Tool::Rclone, OpKind::Copy);
        r.dry_run = true;
        assert!(r.build_argv().unwrap().contains(&"--dry-run".to_string()));

        let mut s = spec(Tool::Rsync, OpKind::Copy);
        s.dry_run = true;
        assert!(s.build_argv().unwrap().contains(&"-n".to_string()));
    }

    #[test]
    fn serde_roundtrip() {
        let s = spec(Tool::Rclone, OpKind::Sync);
        let json = serde_json::to_string(&s).unwrap();
        let back: JobSpec = serde_json::from_str(&json).unwrap();
        assert_eq!(back.op, OpKind::Sync);
        assert_eq!(back.tool, Tool::Rclone);
    }
}
