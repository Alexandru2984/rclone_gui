//! Building argv for `rclone mount` and for unmounting via `fusermount`.
//!
//! A mount is a long-running foreground process: run it through the process
//! runner and keep its handle alive until the user unmounts. Unmounting is done
//! with `fusermount -u <mountpoint>`, which makes the rclone process exit
//! cleanly (we never `kill -9` unless that fails).

use crate::error::{CoreError, Result};

/// The external program used to unmount a FUSE mount on Linux.
pub const UNMOUNT_BIN: &str = "fusermount";

/// Mount tuning. Defaults are conservative and safe.
#[derive(Debug, Clone)]
pub struct MountOptions {
    /// Mount read-only (`--read-only`).
    pub read_only: bool,
    /// Enable a writeback VFS cache (`--vfs-cache-mode writes`), which makes
    /// many apps that need seekable writes work correctly over a remote.
    pub vfs_cache_writes: bool,
}

impl Default for MountOptions {
    fn default() -> Self {
        Self {
            read_only: false,
            vfs_cache_writes: true,
        }
    }
}

/// Build argv for `rclone mount <remote:path> <mountpoint> [flags]`.
pub fn mount_args(remote_path: &str, mountpoint: &str, opts: &MountOptions) -> Result<Vec<String>> {
    if remote_path.trim().is_empty() {
        return Err(CoreError::InvalidCommand("mount source is empty".into()));
    }
    if mountpoint.trim().is_empty() {
        return Err(CoreError::InvalidCommand("mountpoint is empty".into()));
    }

    let mut args = vec![
        "mount".to_string(),
        remote_path.to_string(),
        mountpoint.to_string(),
    ];
    if opts.read_only {
        args.push("--read-only".into());
    }
    if opts.vfs_cache_writes {
        args.push("--vfs-cache-mode".into());
        args.push("writes".into());
    }
    Ok(args)
}

/// Build argv for `fusermount -u <mountpoint>` (the binary is [`UNMOUNT_BIN`]).
pub fn unmount_args(mountpoint: &str) -> Vec<String> {
    vec!["-u".to_string(), mountpoint.to_string()]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basic_mount_argv() {
        let args = mount_args(
            "gdrive:Photos",
            "/home/u/mnt",
            &MountOptions {
                read_only: false,
                vfs_cache_writes: false,
            },
        )
        .unwrap();
        assert_eq!(args, vec!["mount", "gdrive:Photos", "/home/u/mnt"]);
    }

    #[test]
    fn read_only_and_vfs_flags() {
        let args = mount_args(
            "r:",
            "/mnt",
            &MountOptions {
                read_only: true,
                vfs_cache_writes: true,
            },
        )
        .unwrap();
        assert!(args.contains(&"--read-only".to_string()));
        let i = args.iter().position(|a| a == "--vfs-cache-mode").unwrap();
        assert_eq!(args[i + 1], "writes");
    }

    #[test]
    fn empty_endpoints_rejected() {
        let o = MountOptions::default();
        assert!(mount_args("", "/mnt", &o).is_err());
        assert!(mount_args("r:", "  ", &o).is_err());
    }

    #[test]
    fn unmount_argv() {
        assert_eq!(unmount_args("/home/u/mnt"), vec!["-u", "/home/u/mnt"]);
    }
}
