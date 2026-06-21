//! rclone command builder.
//!
//! Turns a typed options struct into an explicit **argv** (`Vec<String>`).
//! There is no code path that produces a shell string for execution; the only
//! string form is [`preview`], which is for *display only* and clearly labelled.

use crate::error::{CoreError, Result};
use crate::security::destructive::Operation;

/// The rclone operation to run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RcloneOp {
    Copy,
    Sync,
    Move,
    Check,
    Size,
    Ls,
    Lsd,
}

impl RcloneOp {
    fn subcommand(self) -> &'static str {
        match self {
            RcloneOp::Copy => "copy",
            RcloneOp::Sync => "sync",
            RcloneOp::Move => "move",
            RcloneOp::Check => "check",
            RcloneOp::Size => "size",
            RcloneOp::Ls => "ls",
            RcloneOp::Lsd => "lsd",
        }
    }

    /// Whether this op takes a destination argument.
    fn takes_dest(self) -> bool {
        matches!(
            self,
            RcloneOp::Copy | RcloneOp::Sync | RcloneOp::Move | RcloneOp::Check
        )
    }

    /// Map to the generic risk-classification operation.
    pub fn as_operation(self) -> Operation {
        match self {
            RcloneOp::Copy => Operation::Copy,
            RcloneOp::Sync => Operation::Sync,
            RcloneOp::Move => Operation::Move,
            RcloneOp::Check => Operation::Check,
            RcloneOp::Size => Operation::Size,
            RcloneOp::Ls | RcloneOp::Lsd => Operation::Ls,
        }
    }
}

/// Tunable options. Defaults are conservative and read-only-ish.
#[derive(Debug, Clone)]
pub struct RcloneOptions {
    pub dry_run: bool,
    pub transfers: Option<u32>,
    pub checkers: Option<u32>,
    pub checksum: bool,
    /// e.g. "10M" — passed verbatim as a single argv item, never shell-expanded.
    pub bwlimit: Option<String>,
    pub retries: Option<u32>,
    pub excludes: Vec<String>,
    pub includes: Vec<String>,
    /// 0 = quiet, 1 = -v, 2 = -vv.
    pub verbosity: u8,
    /// Emit periodic one-line transfer stats for progress parsing.
    pub stats: bool,
    /// Already-tokenized extra flags (each element is one argv item).
    pub extra_flags: Vec<String>,
}

impl Default for RcloneOptions {
    fn default() -> Self {
        Self {
            dry_run: false,
            transfers: None,
            checkers: None,
            checksum: false,
            bwlimit: None,
            retries: None,
            excludes: Vec::new(),
            includes: Vec::new(),
            verbosity: 0,
            stats: true,
            extra_flags: Vec::new(),
        }
    }
}

/// Build the argv for an rclone invocation.
///
/// `source`/`dest` must already be validated by `security::path` for local
/// endpoints. This function additionally refuses empty endpoints.
pub fn build_args(
    op: RcloneOp,
    source: &str,
    dest: Option<&str>,
    opts: &RcloneOptions,
) -> Result<Vec<String>> {
    if source.trim().is_empty() {
        return Err(CoreError::InvalidCommand("source is empty".into()));
    }
    if op.takes_dest() {
        match dest {
            None => {
                return Err(CoreError::InvalidCommand(
                    "operation requires a destination".into(),
                ))
            }
            Some(d) if d.trim().is_empty() => {
                return Err(CoreError::InvalidCommand("destination is empty".into()))
            }
            _ => {}
        }
    }

    let mut args: Vec<String> = vec![op.subcommand().to_string()];
    args.push(source.to_string());
    if op.takes_dest() {
        // safe: presence checked above
        args.push(dest.unwrap().to_string());
    }

    if opts.dry_run {
        args.push("--dry-run".into());
    }
    if let Some(t) = opts.transfers {
        args.push("--transfers".into());
        args.push(t.to_string());
    }
    if let Some(c) = opts.checkers {
        args.push("--checkers".into());
        args.push(c.to_string());
    }
    if opts.checksum {
        args.push("--checksum".into());
    }
    if let Some(b) = &opts.bwlimit {
        args.push("--bwlimit".into());
        args.push(b.clone());
    }
    if let Some(r) = opts.retries {
        args.push("--retries".into());
        args.push(r.to_string());
    }
    for ex in &opts.excludes {
        args.push("--exclude".into());
        args.push(ex.clone());
    }
    for inc in &opts.includes {
        args.push("--include".into());
        args.push(inc.clone());
    }
    match opts.verbosity {
        0 => {}
        1 => args.push("-v".into()),
        _ => args.push("-vv".into()),
    }
    if opts.stats {
        // periodic, parseable one-line progress on stderr
        args.push("--stats".into());
        args.push("1s".into());
        args.push("--stats-one-line".into());
    }
    for flag in &opts.extra_flags {
        args.push(flag.clone());
    }

    Ok(args)
}

/// Render an argv as a copy-pasteable, shell-quoted string **for display only**.
/// This is never executed; execution always uses the argv vector directly.
pub fn preview(binary: &str, args: &[String]) -> String {
    let mut out = String::from(binary);
    for a in args {
        out.push(' ');
        out.push_str(&shell_quote(a));
    }
    out
}

fn shell_quote(s: &str) -> String {
    if !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_./:=@%+".contains(c))
    {
        s.to_string()
    } else {
        // single-quote and escape embedded single quotes
        format!("'{}'", s.replace('\'', r"'\''"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copy_builds_expected_argv() {
        let opts = RcloneOptions {
            stats: false,
            ..Default::default()
        };
        let args = build_args(RcloneOp::Copy, "/src", Some("gdrive:backup"), &opts).unwrap();
        assert_eq!(args, vec!["copy", "/src", "gdrive:backup"]);
    }

    #[test]
    fn dry_run_flag_is_inserted() {
        let opts = RcloneOptions {
            dry_run: true,
            stats: false,
            ..Default::default()
        };
        let args = build_args(RcloneOp::Sync, "/src", Some("/dst"), &opts).unwrap();
        assert!(args.contains(&"--dry-run".to_string()));
    }

    #[test]
    fn missing_destination_is_rejected() {
        let opts = RcloneOptions::default();
        let err = build_args(RcloneOp::Copy, "/src", None, &opts).unwrap_err();
        assert!(matches!(err, CoreError::InvalidCommand(_)));
    }

    #[test]
    fn empty_source_is_rejected() {
        let opts = RcloneOptions::default();
        assert!(build_args(RcloneOp::Size, "   ", None, &opts).is_err());
    }

    #[test]
    fn read_only_op_has_no_dest() {
        let opts = RcloneOptions {
            stats: false,
            ..Default::default()
        };
        let args = build_args(RcloneOp::Size, "gdrive:", None, &opts).unwrap();
        assert_eq!(args, vec!["size", "gdrive:"]);
    }

    #[test]
    fn options_map_to_flags() {
        let opts = RcloneOptions {
            transfers: Some(8),
            checkers: Some(16),
            checksum: true,
            bwlimit: Some("10M".into()),
            retries: Some(5),
            excludes: vec!["*.tmp".into()],
            includes: vec!["*.jpg".into()],
            verbosity: 2,
            stats: false,
            dry_run: false,
            extra_flags: vec!["--fast-list".into()],
        };
        let args = build_args(RcloneOp::Sync, "/a", Some("/b"), &opts).unwrap();
        let joined = args.join(" ");
        assert!(joined.contains("--transfers 8"));
        assert!(joined.contains("--checkers 16"));
        assert!(joined.contains("--checksum"));
        assert!(joined.contains("--bwlimit 10M"));
        assert!(joined.contains("--retries 5"));
        assert!(joined.contains("--exclude *.tmp"));
        assert!(joined.contains("--include *.jpg"));
        assert!(joined.contains("-vv"));
        assert!(joined.contains("--fast-list"));
    }

    #[test]
    fn preview_quotes_dangerous_chars_but_is_display_only() {
        let args = vec!["sync".into(), "/path with space".into(), "/dst".into()];
        let p = preview("rclone", &args);
        assert_eq!(p, "rclone sync '/path with space' /dst");
    }
}
