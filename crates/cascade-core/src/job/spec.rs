//! A tool-agnostic job specification.
//!
//! `JobSpec` is the bridge between the UI's intent and the concrete argv passed
//! to rclone/rsync. It also exposes the operation's [`RiskLevel`] so the UI can
//! gate destructive runs. This type is fully unit-testable without a display.

use serde::{Deserialize, Serialize};

use crate::error::{CoreError, Result};
use crate::rclone::command::{self as rclone_cmd, RcloneOp, RcloneOptions};
use crate::rsync::command::{build_args as rsync_args, RsyncOptions};
use crate::security::destructive::{classify, Operation, RiskLevel};
use crate::security::sanitize;
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
    /// Two-way sync (rclone `bisync`); keeps both sides in step. rclone only.
    Bisync,
}

impl OpKind {
    fn as_operation(self) -> Operation {
        match self {
            OpKind::Copy => Operation::Copy,
            OpKind::Sync => Operation::Sync,
            OpKind::Move => Operation::Move,
            OpKind::Bisync => Operation::Bisync,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            OpKind::Copy => "Copy",
            OpKind::Sync => "Sync (mirror)",
            OpKind::Move => "Move",
            OpKind::Bisync => "Bisync (two-way)",
        }
    }
}

/// Advanced (power-user) options. Each maps to the relevant tool's flags;
/// options that don't apply to the chosen tool are simply ignored.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct AdvancedOptions {
    pub excludes: Vec<String>,
    pub includes: Vec<String>,
    /// rclone `--transfers`.
    pub transfers: Option<u32>,
    /// rclone `--checkers`.
    pub checkers: Option<u32>,
    /// rclone `--bwlimit` (e.g. "10M"); a single argv item, never shell-expanded.
    pub bwlimit: Option<String>,
    /// rclone `--retries`.
    pub retries: Option<u32>,
    /// Abort if the run would delete more than this many files (rclone
    /// `--max-delete`, rsync `--max-delete=N`). A runaway-mirror safety net.
    pub max_delete: Option<u64>,
    /// Move replaced/deleted files here instead of removing them (rclone
    /// `--backup-dir`, rsync `--backup --backup-dir=`). Reversible sync.
    pub backup_dir: Option<String>,
    /// Verify by checksum (rclone `--checksum`, rsync `--checksum`).
    pub checksum: bool,
    /// rsync `-z` compression.
    pub compress: bool,
    /// rsync SSH transport port (`-e "ssh -p N"`).
    pub ssh_port: Option<u16>,
    /// bisync only: establish the baseline on the first run (rclone `--resync`).
    pub resync: bool,
    /// Already-tokenized custom flags (validated by `security::flags`).
    pub extra_flags: Vec<String>,
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
    /// Power-user options. Defaulted so older serialized profiles still load.
    #[serde(default)]
    pub options: AdvancedOptions,
}

impl JobSpec {
    /// Whether deletion of destination files actually happens for this spec.
    /// `Sync` mirrors (both tools), so it always deletes extras at the dest.
    pub fn delete_effective(&self) -> bool {
        match self.op {
            OpKind::Sync => true,
            OpKind::Move => true,
            OpKind::Bisync => true,
            OpKind::Copy => self.delete,
        }
    }

    /// The risk level the UI uses to decide on confirmation + dry-run defaults.
    ///
    /// Custom flags are also inspected: a `Copy` job is normally only `Caution`,
    /// but if the user added a deletion flag (e.g. `--delete`) or a remote-exec
    /// flag (rsync `-e` / `--rsync-path`) via Advanced, it is escalated to
    /// `Destructive` so the confirmation gate still applies.
    pub fn risk(&self) -> RiskLevel {
        let base = classify(self.op.as_operation(), self.delete_effective());
        if base == RiskLevel::Destructive || self.has_dangerous_flags() {
            RiskLevel::Destructive
        } else {
            base
        }
    }

    /// Whether the custom flags can delete data or run a command of the user's
    /// (or a remote's) choosing. Kept deliberately broad — over-escalating to a
    /// confirmation prompt is cheap; missing a data-losing flag is not.
    fn has_dangerous_flags(&self) -> bool {
        self.options.extra_flags.iter().any(|f| {
            f.starts_with("--delete")               // rclone/rsync delete-* variants
                || f == "--del"                      // rsync alias of --delete-during
                || f.starts_with("--remove-source-files")
                || f == "--remove-sent-files"        // older rsync alias
                || f == "--rsync-path"
                || f.starts_with("--rsync-path=")
                || f == "-e"
                || f.starts_with("--rsh")
                || f.starts_with("--password-command") // rclone: runs an arbitrary command
        })
    }

    /// The binary that will be invoked.
    pub fn binary(&self) -> &'static str {
        self.tool.binary()
    }

    /// Build the concrete argv. Never produces a shell string.
    pub fn build_argv(&self) -> Result<Vec<String>> {
        let o = &self.options;
        match self.tool {
            Tool::Rclone => {
                let op = match self.op {
                    OpKind::Copy => RcloneOp::Copy,
                    OpKind::Sync => RcloneOp::Sync,
                    OpKind::Move => RcloneOp::Move,
                    OpKind::Bisync => RcloneOp::Bisync,
                };
                let opts = RcloneOptions {
                    dry_run: self.dry_run,
                    transfers: o.transfers,
                    checkers: o.checkers,
                    checksum: o.checksum,
                    bwlimit: o.bwlimit.clone(),
                    retries: o.retries,
                    max_delete: o.max_delete,
                    backup_dir: o.backup_dir.clone(),
                    excludes: o.excludes.clone(),
                    includes: o.includes.clone(),
                    resync: o.resync,
                    extra_flags: o.extra_flags.clone(),
                    ..Default::default()
                };
                rclone_cmd::build_args(op, &self.source, Some(&self.destination), &opts)
            }
            Tool::Rsync => {
                if self.op == OpKind::Bisync {
                    return Err(CoreError::InvalidCommand(
                        "two-way sync (bisync) is available with rclone only".into(),
                    ));
                }
                let mut extra_flags = o.extra_flags.clone();
                if o.checksum {
                    extra_flags.push("--checksum".into());
                }
                let mut opts = RsyncOptions {
                    dry_run: self.dry_run,
                    delete: self.delete_effective(),
                    compress: o.compress,
                    max_delete: o.max_delete,
                    backup_dir: o.backup_dir.clone(),
                    excludes: o.excludes.clone(),
                    includes: o.includes.clone(),
                    ssh_port: o.ssh_port,
                    extra_flags,
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

    /// Like [`preview`], but with secrets redacted. Use this for anything that
    /// is **persisted or shown after the fact** (history, on-disk logs,
    /// clipboard) so credentials embedded in paths or flags never leak at rest.
    pub fn preview_sanitized(&self) -> Result<String> {
        Ok(sanitize::redact(&self.preview()?))
    }

    /// Whether this spec embeds something the sanitizer recognizes as a secret
    /// (a password in a connection string, a `--*-pass` flag, a credential URL).
    /// Used to refuse persisting secrets into a profile — Cascade is never a
    /// credential store; users should configure an rclone remote instead.
    pub fn contains_secret(&self) -> bool {
        let joined = format!(
            "{} {} {}",
            self.source,
            self.destination,
            self.options.extra_flags.join(" ")
        );
        sanitize::redact(&joined) != joined
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
            options: AdvancedOptions::default(),
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
        assert_eq!(
            spec(Tool::Rclone, OpKind::Sync).risk(),
            RiskLevel::Destructive
        );
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
    fn rclone_advanced_options_map_to_flags() {
        let mut s = spec(Tool::Rclone, OpKind::Copy);
        s.options = AdvancedOptions {
            excludes: vec!["*.tmp".into()],
            transfers: Some(8),
            bwlimit: Some("10M".into()),
            checksum: true,
            extra_flags: vec!["--fast-list".into()],
            ..Default::default()
        };
        let joined = s.build_argv().unwrap().join(" ");
        assert!(joined.contains("--exclude *.tmp"));
        assert!(joined.contains("--transfers 8"));
        assert!(joined.contains("--bwlimit 10M"));
        assert!(joined.contains("--checksum"));
        assert!(joined.contains("--fast-list"));
    }

    #[test]
    fn rsync_advanced_options_map_to_flags() {
        let mut s = spec(Tool::Rsync, OpKind::Copy);
        s.options = AdvancedOptions {
            excludes: vec![".git".into()],
            compress: true,
            checksum: true,
            ssh_port: Some(2222),
            ..Default::default()
        };
        let argv = s.build_argv().unwrap();
        let joined = argv.join(" ");
        assert!(joined.contains("--exclude .git"));
        assert!(argv.contains(&"-z".to_string()));
        assert!(argv.contains(&"--checksum".to_string()));
        assert!(joined.contains("ssh -p 2222"));
    }

    #[test]
    fn rclone_bisync_builds_bisync_argv_and_is_destructive() {
        let mut s = spec(Tool::Rclone, OpKind::Bisync);
        s.options.resync = true;
        let argv = s.build_argv().unwrap();
        assert_eq!(argv[0], "bisync");
        assert_eq!(&argv[1..3], &["/src/".to_string(), "/dst/".to_string()]);
        assert!(argv.contains(&"--resync".to_string()));
        assert_eq!(s.risk(), RiskLevel::Destructive);
    }

    #[test]
    fn rsync_bisync_is_rejected() {
        let s = spec(Tool::Rsync, OpKind::Bisync);
        assert!(s.build_argv().is_err());
    }

    #[test]
    fn resync_only_applies_to_bisync() {
        // --resync must not leak onto a plain sync even if the flag is set.
        let mut s = spec(Tool::Rclone, OpKind::Sync);
        s.options.resync = true;
        assert!(!s.build_argv().unwrap().contains(&"--resync".to_string()));
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
    fn copy_with_delete_flag_escalates_to_destructive() {
        let mut s = spec(Tool::Rsync, OpKind::Copy);
        assert_eq!(s.risk(), RiskLevel::Caution);
        s.options.extra_flags = vec!["--delete".into()];
        assert_eq!(s.risk(), RiskLevel::Destructive);
    }

    #[test]
    fn remote_exec_flags_escalate_to_destructive() {
        for flag in ["-e", "--rsync-path=/usr/bin/evil"] {
            let mut s = spec(Tool::Rsync, OpKind::Copy);
            s.options.extra_flags = vec![flag.into()];
            assert_eq!(s.risk(), RiskLevel::Destructive, "{flag} should escalate");
        }
    }

    #[test]
    fn delete_aliases_and_command_flags_escalate() {
        for flag in [
            "--del",
            "--remove-sent-files",
            "--password-command=/bin/echo x",
        ] {
            let mut s = spec(Tool::Rsync, OpKind::Copy);
            s.options.extra_flags = vec![flag.into()];
            assert_eq!(s.risk(), RiskLevel::Destructive, "{flag} should escalate");
        }
    }

    #[test]
    fn harmless_flags_do_not_escalate() {
        let mut s = spec(Tool::Rclone, OpKind::Copy);
        s.options.extra_flags = vec!["--fast-list".into(), "--transfers".into(), "8".into()];
        assert_eq!(s.risk(), RiskLevel::Caution);
    }

    #[test]
    fn contains_secret_detects_embedded_credentials() {
        let mut s = spec(Tool::Rsync, OpKind::Copy);
        assert!(!s.contains_secret());
        s.options.extra_flags = vec!["--sftp-pass".into(), "hunter2".into()];
        assert!(s.contains_secret());

        let mut u = spec(Tool::Rclone, OpKind::Copy);
        u.source = "https://alice:s3cr3t@example.com".into();
        assert!(u.contains_secret());
    }

    #[test]
    fn preview_sanitized_redacts_secrets_in_flags() {
        let mut s = spec(Tool::Rsync, OpKind::Copy);
        s.options.extra_flags = vec!["--sftp-pass".into(), "hunter2".into()];
        let raw = s.preview().unwrap();
        let safe = s.preview_sanitized().unwrap();
        assert!(raw.contains("hunter2"), "raw preview keeps the secret");
        assert!(
            !safe.contains("hunter2"),
            "sanitized preview must redact it"
        );
    }

    #[test]
    fn serde_roundtrip() {
        let s = spec(Tool::Rclone, OpKind::Sync);
        let json = serde_json::to_string(&s).unwrap();
        let back: JobSpec = serde_json::from_str(&json).unwrap();
        assert_eq!(back.op, OpKind::Sync);
        assert_eq!(back.tool, Tool::Rclone);
    }

    #[test]
    fn old_profile_without_options_still_loads() {
        // A spec serialized before AdvancedOptions existed (no `options` key).
        let json = r#"{
            "name":"legacy","tool":"rsync","op":"copy",
            "source":"/a/","destination":"/b/","dry_run":false,"delete":false
        }"#;
        let back: JobSpec = serde_json::from_str(json).unwrap();
        assert_eq!(back.name, "legacy");
        assert!(back.options.excludes.is_empty());
        assert_eq!(back.options.max_delete, None);
        assert!(!back.options.resync);
    }

    #[test]
    fn old_options_without_new_fields_default_them() {
        // An options blob predating max_delete/backup_dir/resync.
        let json = r#"{
            "name":"legacy","tool":"rclone","op":"sync",
            "source":"/a/","destination":"gdrive:b","dry_run":false,"delete":false,
            "options":{"excludes":["*.tmp"],"transfers":4}
        }"#;
        let back: JobSpec = serde_json::from_str(json).unwrap();
        assert_eq!(back.options.excludes, vec!["*.tmp".to_string()]);
        assert_eq!(back.options.transfers, Some(4));
        assert_eq!(back.options.max_delete, None);
        assert_eq!(back.options.backup_dir, None);
        assert!(!back.options.resync);
    }

    #[test]
    fn delete_effective_matches_operation() {
        assert!(!spec(Tool::Rsync, OpKind::Copy).delete_effective());
        assert!(spec(Tool::Rsync, OpKind::Sync).delete_effective());
        assert!(spec(Tool::Rsync, OpKind::Move).delete_effective());
        assert!(spec(Tool::Rclone, OpKind::Bisync).delete_effective());
        // Copy + explicit delete flag flips it on.
        let mut s = spec(Tool::Rsync, OpKind::Copy);
        s.delete = true;
        assert!(s.delete_effective());
    }

    #[test]
    fn preview_is_shell_quoted_and_starts_with_binary() {
        let mut s = spec(Tool::Rsync, OpKind::Copy);
        s.source = "/path with space/".into();
        let p = s.preview().unwrap();
        assert!(p.starts_with("rsync "));
        assert!(p.contains("'/path with space/'"));
    }

    #[test]
    fn labels_cover_every_opkind() {
        for op in [OpKind::Copy, OpKind::Sync, OpKind::Move, OpKind::Bisync] {
            assert!(!op.label().is_empty());
        }
    }
}
