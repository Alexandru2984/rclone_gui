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
use crate::security::path::{self, PathVerdict};
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
    /// Validate every path-like field against the selected tool and current
    /// filesystem state. Callers should display the returned warnings and keep
    /// them as a safety snapshot for delayed execution.
    pub fn validate_paths(&self) -> Result<Vec<String>> {
        let mut warnings = Vec::new();
        for (label, endpoint) in [
            ("source", self.source.as_str()),
            ("destination", self.destination.as_str()),
        ] {
            validate_endpoint_text(label, endpoint)?;
            if path::is_remote_endpoint(endpoint) {
                if self.tool == Tool::Rsync && path::looks_like_rclone_remote(endpoint) {
                    return Err(CoreError::InvalidPath(format!(
                        "{label} '{endpoint}' looks like an rclone remote, not an rsync/SSH endpoint"
                    )));
                }
                if self.tool == Tool::Rclone && !path::looks_like_rclone_remote(endpoint) {
                    return Err(CoreError::InvalidPath(format!(
                        "{label} '{endpoint}' looks like an rsync/SSH endpoint, not an rclone remote"
                    )));
                }
                validate_remote_path(label, endpoint, &mut warnings)?;
            } else if !is_rclone_backend_endpoint(self.tool, endpoint) {
                match path::validate(endpoint)? {
                    PathVerdict::Ok => {}
                    PathVerdict::Warn(warning) => {
                        warnings.push(format!("{label}: {warning}"));
                    }
                }
            }
        }

        if let Some(backup_dir) = self.options.backup_dir.as_deref() {
            validate_endpoint_text("backup directory", backup_dir)?;
            if path::is_remote_endpoint(backup_dir) {
                if self.tool == Tool::Rsync {
                    return Err(CoreError::InvalidPath(
                        "rsync backup directory must be a local path on the receiving side".into(),
                    ));
                }
                if !path::looks_like_rclone_remote(backup_dir) {
                    return Err(CoreError::InvalidPath(format!(
                        "backup directory '{backup_dir}' is not an rclone remote"
                    )));
                }
                validate_remote_path("backup directory", backup_dir, &mut warnings)?;
            } else if !is_rclone_backend_endpoint(self.tool, backup_dir) {
                match path::validate(backup_dir)? {
                    PathVerdict::Ok => {}
                    PathVerdict::Warn(warning) => {
                        warnings.push(format!("backup directory: {warning}"));
                    }
                }
            }
            if let Some(warning) = path::check_overlap(&self.destination, backup_dir).warning() {
                warnings.push(format!("backup directory: {warning}"));
            }
        }

        if let Some(warning) = path::check_overlap(&self.source, &self.destination).warning() {
            warnings.push(warning.to_string());
        }
        Ok(warnings)
    }

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

    /// The rclone [`RcloneOp`] for this spec's operation.
    pub fn rclone_op(&self) -> RcloneOp {
        match self.op {
            OpKind::Copy => RcloneOp::Copy,
            OpKind::Sync => RcloneOp::Sync,
            OpKind::Move => RcloneOp::Move,
            OpKind::Bisync => RcloneOp::Bisync,
        }
    }

    /// Map this spec's advanced options onto [`RcloneOptions`]. Shared by the
    /// argv builder and the RC (Remote Control) payload builder so both stay in
    /// step.
    pub fn rclone_options(&self) -> RcloneOptions {
        let o = &self.options;
        RcloneOptions {
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
        }
    }

    /// Validate and resolve local endpoints into stable absolute spellings.
    /// Existing symlinks are removed so every execution path (CLI, RC, or a
    /// generated service) can use the same safety snapshot.
    pub fn resolved_for_execution(&self) -> Result<Self> {
        self.ensure_no_embedded_secrets()?;
        let _ = self.validate_paths()?;
        let mut resolved = self.clone();
        resolved.source = endpoint_for_argv(self.tool, &self.source)?;
        resolved.destination = endpoint_for_argv(self.tool, &self.destination)?;
        resolved.options.backup_dir = self
            .options
            .backup_dir
            .as_deref()
            .map(|dir| endpoint_for_argv(self.tool, dir))
            .transpose()?;
        Ok(resolved)
    }

    /// Build the concrete argv. Never produces a shell string.
    pub fn build_argv(&self) -> Result<Vec<String>> {
        self.prepare_execution().map(|(_, argv)| argv)
    }

    /// Return one resolved spec and the exact argv derived from it. Callers
    /// that have multiple execution backends (CLI/RC/systemd) use this to avoid
    /// resolving the same symlink to different targets in adjacent steps.
    pub fn prepare_execution(&self) -> Result<(Self, Vec<String>)> {
        let resolved = self.resolved_for_execution()?;
        let argv = resolved.build_resolved_argv()?;
        Ok((resolved, argv))
    }

    fn build_resolved_argv(&self) -> Result<Vec<String>> {
        let resolved = self;
        let o = &resolved.options;
        match resolved.tool {
            Tool::Rclone => {
                let opts = resolved.rclone_options();
                rclone_cmd::build_args(
                    resolved.rclone_op(),
                    &resolved.source,
                    Some(&resolved.destination),
                    &opts,
                )
            }
            Tool::Rsync => {
                if resolved.op == OpKind::Bisync {
                    return Err(CoreError::InvalidCommand(
                        "two-way sync (bisync) is available with rclone only".into(),
                    ));
                }
                let mut opts = RsyncOptions {
                    dry_run: resolved.dry_run,
                    delete: resolved.delete_effective(),
                    compress: o.compress,
                    checksum: o.checksum,
                    max_delete: o.max_delete,
                    backup_dir: o.backup_dir.clone(),
                    excludes: o.excludes.clone(),
                    includes: o.includes.clone(),
                    ssh_port: o.ssh_port,
                    extra_flags: o.extra_flags.clone(),
                    ..Default::default()
                };
                // rsync has no `move`; emulate it with --remove-source-files.
                if resolved.op == OpKind::Move {
                    opts.delete = false; // moving is not mirroring
                    opts.remove_source_files = true;
                }
                rsync_args(&resolved.source, &resolved.destination, &opts)
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

    /// Sanitize a preview from the exact argv snapshot that will be spawned.
    /// This avoids rebuilding paths after validation and accidentally showing a
    /// different symlink target than the command actually receives.
    pub fn preview_argv_sanitized(&self, argv: &[String]) -> String {
        sanitize::redact(&rclone_cmd::preview(self.binary(), argv))
    }

    /// Whether any serialized field embeds something the sanitizer recognizes
    /// as a secret (connection-string credentials, provider flags, tokens,
    /// private keys, and common config key/value forms).
    pub fn contains_secret(&self) -> bool {
        let serialized_secret = serde_json::to_string(self)
            .is_ok_and(|serialized| sanitize::contains_secret(&serialized));
        let joined_flags = self.options.extra_flags.join(" ");
        serialized_secret || sanitize::contains_secret(&joined_flags)
    }

    /// Reject credentials embedded in a job before they can reach argv, the
    /// process list, logs, history, profiles, or the persisted queue.
    pub fn ensure_no_embedded_secrets(&self) -> Result<()> {
        if self.contains_secret() {
            return Err(CoreError::InvalidCommand(
                "this job embeds a credential; configure an rclone remote or an SSH agent/key \
                 outside Cascade, then reference it without putting the secret in the job"
                    .into(),
            ));
        }
        Ok(())
    }
}

const MAX_ENDPOINT_BYTES: usize = 4096;

fn validate_endpoint_text(label: &str, endpoint: &str) -> Result<()> {
    if endpoint.trim() != endpoint {
        return Err(CoreError::InvalidPath(format!(
            "{label} has leading or trailing whitespace"
        )));
    }
    if endpoint.len() > MAX_ENDPOINT_BYTES {
        return Err(CoreError::InvalidPath(format!(
            "{label} exceeds the {MAX_ENDPOINT_BYTES}-byte safety limit"
        )));
    }
    if endpoint.chars().any(char::is_control) {
        return Err(CoreError::InvalidPath(format!(
            "{label} contains a control character"
        )));
    }
    Ok(())
}

fn validate_remote_path(label: &str, endpoint: &str, warnings: &mut Vec<String>) -> Result<()> {
    let remote_path = endpoint.split_once(':').map_or("", |(_, path)| path);
    if remote_path.split('/').any(|component| component == "..") {
        return Err(CoreError::DangerousPath(format!(
            "{label} remote path must not contain '..'"
        )));
    }
    if remote_path.is_empty() || remote_path.chars().all(|character| character == '/') {
        warnings.push(format!(
            "{label} is the root of a remote; this can affect every object on that remote"
        ));
    }
    Ok(())
}

fn is_rclone_backend_endpoint(tool: Tool, endpoint: &str) -> bool {
    tool == Tool::Rclone && endpoint.starts_with(':') && endpoint[1..].contains(':')
}

fn endpoint_for_argv(tool: Tool, endpoint: &str) -> Result<String> {
    if path::is_remote_endpoint(endpoint) || is_rclone_backend_endpoint(tool, endpoint) {
        Ok(endpoint.to_string())
    } else {
        path::resolve_for_execution(endpoint)
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
        assert_eq!(
            &argv[argv.len() - 2..],
            &["/src/".to_string(), "/dst/".to_string()]
        );
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
        assert_eq!(
            &argv[argv.len() - 2..],
            &["/src/".to_string(), "/dst/".to_string()]
        );
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
    fn custom_flags_cannot_negate_dry_run_or_delete_guard() {
        let mut s = spec(Tool::Rclone, OpKind::Sync);
        s.dry_run = true;
        s.options.max_delete = Some(1);
        for bypass in ["--dry-run=false", "--no-dry-run", "--max-delete=-1"] {
            s.options.extra_flags = vec![bypass.into()];
            assert!(s.build_argv().is_err(), "accepted bypass: {bypass}");
        }
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
        s.options.extra_flags = vec!["--fast-list".into(), "--metadata-set=x=y".into()];
        assert_eq!(s.risk(), RiskLevel::Caution);
    }

    #[test]
    fn contains_secret_detects_embedded_credentials() {
        let mut s = spec(Tool::Rsync, OpKind::Copy);
        assert!(!s.contains_secret());
        s.options.extra_flags = vec!["--sftp-pass=hunter2".into()];
        assert!(s.contains_secret());

        let mut u = spec(Tool::Rclone, OpKind::Copy);
        u.source = "https://alice:s3cr3t@example.com".into();
        assert!(u.contains_secret());

        let mut provider = spec(Tool::Rclone, OpKind::Copy);
        provider.options.extra_flags = vec!["--s3-secret-access-key=aws-secret".into()];
        assert!(provider.contains_secret());

        let mut nested = spec(Tool::Rclone, OpKind::Copy);
        nested.options.excludes = vec!["pass=hidden-in-an-option".into()];
        assert!(nested.contains_secret());
    }

    #[test]
    fn secret_bearing_jobs_cannot_build_or_preview() {
        let mut s = spec(Tool::Rsync, OpKind::Copy);
        s.options.extra_flags = vec!["--sftp-pass=hunter2".into()];
        assert!(s.build_argv().is_err());
        assert!(s.preview().is_err());
        assert!(s.preview_sanitized().is_err());
    }

    #[test]
    fn dangerous_paths_are_rejected_by_the_argv_boundary() {
        for tool in [Tool::Rclone, Tool::Rsync] {
            let mut s = spec(tool, OpKind::Copy);
            s.destination = "/".into();
            assert!(s.build_argv().is_err());

            let mut backup = spec(tool, OpKind::Sync);
            backup.options.backup_dir = Some("/".into());
            assert!(backup.build_argv().is_err());
        }
    }

    #[test]
    fn filesystem_paths_are_revalidated_after_symlink_changes() {
        #[cfg(unix)]
        {
            let dir = tempfile::tempdir().unwrap();
            let safe = dir.path().join("safe");
            let link = dir.path().join("link");
            std::fs::create_dir(&safe).unwrap();
            std::os::unix::fs::symlink(&safe, &link).unwrap();
            let mut s = spec(Tool::Rsync, OpKind::Copy);
            s.source = link.to_string_lossy().into_owned();
            let (resolved, argv) = s.prepare_execution().unwrap();
            assert_eq!(resolved.source, safe.to_string_lossy());
            assert!(argv.contains(&safe.to_string_lossy().into_owned()));
            assert!(!argv.contains(&link.to_string_lossy().into_owned()));

            std::fs::remove_file(&link).unwrap();
            std::os::unix::fs::symlink("/", &link).unwrap();
            assert!(s.build_argv().is_err());
        }
    }

    #[test]
    fn path_warnings_cover_remote_roots_system_dirs_and_overlap() {
        let mut remote_root = spec(Tool::Rclone, OpKind::Sync);
        remote_root.source = "source:data".into();
        remote_root.destination = "backup:".into();
        assert!(remote_root
            .validate_paths()
            .unwrap()
            .iter()
            .any(|warning| warning.contains("root of a remote")));

        let mut system = spec(Tool::Rsync, OpKind::Copy);
        system.source = "/etc".into();
        assert!(system
            .validate_paths()
            .unwrap()
            .iter()
            .any(|warning| warning.contains("system directory")));

        let mut overlap = spec(Tool::Rsync, OpKind::Copy);
        overlap.destination = overlap.source.clone();
        assert!(overlap
            .validate_paths()
            .unwrap()
            .iter()
            .any(|warning| warning.contains("same location")));
    }

    #[test]
    fn endpoints_reject_control_whitespace_traversal_and_tool_mismatch() {
        let mut s = spec(Tool::Rclone, OpKind::Copy);
        for source in [" /src", "/src\n", "remote:../escape", "user@host:/src"] {
            s.source = source.into();
            assert!(s.validate_paths().is_err(), "accepted {source:?}");
        }

        let mut r = spec(Tool::Rsync, OpKind::Copy);
        r.source = "gdrive:data".into();
        assert!(r.validate_paths().is_err());
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
