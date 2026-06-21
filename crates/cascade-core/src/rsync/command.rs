//! rsync command builder — explicit argv, never a shell string.
//!
//! Note on trailing slashes: rsync treats `src/` (contents) differently from
//! `src` (the directory itself). Cascade surfaces this to the user in the UI
//! rather than silently rewriting paths.

use crate::error::{CoreError, Result};

/// Options for an rsync transfer.
#[derive(Debug, Clone)]
pub struct RsyncOptions {
    /// `-a`: recurse and preserve perms, times, symlinks, owner/group.
    pub archive: bool,
    /// `-n`: dry-run.
    pub dry_run: bool,
    /// `--delete`: remove files at the destination not present at the source.
    /// Opt-in and destructive — gated by the destructive-op confirmation flow.
    pub delete: bool,
    /// `-z`: compress during transfer.
    pub compress: bool,
    /// `--exclude PATTERN` (repeated).
    pub excludes: Vec<String>,
    /// `--include PATTERN` (repeated).
    pub includes: Vec<String>,
    /// Emit `--info=progress2` line-buffered progress for parsing.
    pub progress: bool,
    /// `-v` count (0..=2).
    pub verbosity: u8,
    /// Remote SSH transport: `Some(("user@host", port))` builds `-e "ssh -p PORT"`.
    /// The source or destination string must itself carry the `user@host:/path`.
    pub ssh_port: Option<u16>,
    /// Already-tokenized extra flags (each element is one argv item).
    pub extra_flags: Vec<String>,
}

impl Default for RsyncOptions {
    fn default() -> Self {
        Self {
            archive: true,
            dry_run: false,
            delete: false,
            compress: false,
            excludes: Vec::new(),
            includes: Vec::new(),
            progress: true,
            verbosity: 1,
            ssh_port: None,
            extra_flags: Vec::new(),
        }
    }
}

/// Build the argv for an rsync invocation: `rsync [flags] SRC DEST`.
pub fn build_args(source: &str, dest: &str, opts: &RsyncOptions) -> Result<Vec<String>> {
    if source.trim().is_empty() {
        return Err(CoreError::InvalidCommand("source is empty".into()));
    }
    if dest.trim().is_empty() {
        return Err(CoreError::InvalidCommand("destination is empty".into()));
    }

    let mut args: Vec<String> = Vec::new();

    if opts.archive {
        args.push("-a".into());
    }
    match opts.verbosity {
        0 => {}
        1 => args.push("-v".into()),
        _ => args.push("-vv".into()),
    }
    if opts.compress {
        args.push("-z".into());
    }
    if opts.dry_run {
        args.push("-n".into());
    }
    if opts.delete {
        args.push("--delete".into());
    }
    if opts.progress {
        // progress2 = overall transfer stats; outbuf=L = line-buffered, parseable
        args.push("--info=progress2".into());
        args.push("--outbuf=L".into());
    }
    for ex in &opts.excludes {
        args.push("--exclude".into());
        args.push(ex.clone());
    }
    for inc in &opts.includes {
        args.push("--include".into());
        args.push(inc.clone());
    }
    if let Some(port) = opts.ssh_port {
        // One argv item; rsync invokes ssh itself (no shell on our side).
        args.push("-e".into());
        args.push(format!("ssh -p {port}"));
    }
    for flag in &opts.extra_flags {
        args.push(flag.clone());
    }

    args.push(source.to_string());
    args.push(dest.to_string());
    Ok(args)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_archive_and_safe() {
        let args = build_args("/src/", "/dst/", &RsyncOptions::default()).unwrap();
        assert!(args.contains(&"-a".to_string()));
        assert!(!args.contains(&"--delete".to_string())); // never implicit
        assert_eq!(args.last().unwrap(), "/dst/");
    }

    #[test]
    fn delete_is_opt_in() {
        let opts = RsyncOptions {
            delete: true,
            ..Default::default()
        };
        let args = build_args("/src/", "/dst/", &opts).unwrap();
        assert!(args.contains(&"--delete".to_string()));
    }

    #[test]
    fn dry_run_flag() {
        let opts = RsyncOptions {
            dry_run: true,
            ..Default::default()
        };
        assert!(build_args("/a", "/b", &opts)
            .unwrap()
            .contains(&"-n".to_string()));
    }

    #[test]
    fn ssh_transport_builds_single_e_arg() {
        let opts = RsyncOptions {
            ssh_port: Some(2222),
            ..Default::default()
        };
        let args = build_args("/local/", "user@host:/remote/", &opts).unwrap();
        let e_idx = args.iter().position(|a| a == "-e").unwrap();
        assert_eq!(args[e_idx + 1], "ssh -p 2222");
    }

    #[test]
    fn empty_endpoints_rejected() {
        assert!(build_args("", "/b", &RsyncOptions::default()).is_err());
        assert!(build_args("/a", "  ", &RsyncOptions::default()).is_err());
    }

    #[test]
    fn source_and_dest_are_last_two_args() {
        let opts = RsyncOptions {
            excludes: vec![".git".into()],
            ..Default::default()
        };
        let args = build_args("/src/", "/dst/", &opts).unwrap();
        let n = args.len();
        assert_eq!(&args[n - 2..], &["/src/".to_string(), "/dst/".to_string()]);
    }
}
