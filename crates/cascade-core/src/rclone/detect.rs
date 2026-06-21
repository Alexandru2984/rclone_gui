//! Detect whether a tool binary is installed and read its version.

use std::process::Command;

/// Result of probing for an external tool.
#[derive(Debug, Clone)]
pub struct ToolInfo {
    pub binary: String,
    pub path: std::path::PathBuf,
    pub version: String,
}

/// Probe for `rclone` on `PATH`. Returns `None` if not installed.
pub fn detect() -> Option<ToolInfo> {
    detect_named("rclone", &["version"])
}

/// Shared detection helper (also used by the rsync module).
pub(crate) fn detect_named(binary: &str, version_args: &[&str]) -> Option<ToolInfo> {
    // `which`-style lookup without an extra dependency: rely on the OS resolving
    // the binary name, then read its version. We never pass user input here.
    let output = Command::new(binary).args(version_args).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let version = String::from_utf8_lossy(&output.stdout)
        .lines()
        .next()
        .unwrap_or("")
        .trim()
        .to_string();

    let path = which(binary).unwrap_or_else(|| std::path::PathBuf::from(binary));
    Some(ToolInfo {
        binary: binary.to_string(),
        path,
        version,
    })
}

/// Minimal `which`: scan `PATH` for an executable `binary`. No shell involved.
fn which(binary: &str) -> Option<std::path::PathBuf> {
    let path_var = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path_var) {
        let candidate = dir.join(binary);
        if let Ok(meta) = std::fs::metadata(&candidate) {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if meta.is_file() && meta.permissions().mode() & 0o111 != 0 {
                    return Some(candidate);
                }
            }
            #[cfg(not(unix))]
            if meta.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}
