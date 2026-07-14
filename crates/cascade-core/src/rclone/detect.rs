//! Detect whether a tool binary is installed and read its version.

use std::io::Read;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use crate::security::sanitize;

const MAX_VERSION_BYTES: usize = 64 * 1024;
const VERSION_TIMEOUT: Duration = Duration::from_secs(5);

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
    detect_named_with_limits(binary, version_args, MAX_VERSION_BYTES, VERSION_TIMEOUT)
}

fn detect_named_with_limits(
    binary: &str,
    version_args: &[&str],
    max_bytes: usize,
    timeout: Duration,
) -> Option<ToolInfo> {
    // `which`-style lookup without an extra dependency: rely on the OS resolving
    // the binary name, then read its version. We never pass user input here.
    let mut command = Command::new(binary);
    command
        .args(version_args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let mut child = command.spawn().ok()?;
    let pid = child.id();
    let stdout = child.stdout.take()?;
    let reader = std::thread::spawn(move || {
        let mut bytes = Vec::with_capacity(max_bytes.min(8192));
        stdout
            .take(max_bytes.saturating_add(1) as u64)
            .read_to_end(&mut bytes)
            .ok()?;
        Some(bytes)
    });

    let started = Instant::now();
    let status = loop {
        match child.try_wait().ok()? {
            Some(status) => break status,
            None if started.elapsed() < timeout => {
                std::thread::sleep(Duration::from_millis(10));
            }
            None => {
                #[cfg(unix)]
                // SAFETY: the child was placed in a dedicated process group.
                unsafe {
                    libc::kill(-(pid as libc::pid_t), libc::SIGKILL);
                }
                let _ = child.kill();
                let _ = child.wait();
                let _ = reader.join();
                return None;
            }
        }
    };
    #[cfg(unix)]
    // Version probes must not leave forked descendants holding stdout open.
    // SAFETY: the child was placed in a dedicated process group.
    unsafe {
        libc::kill(-(pid as libc::pid_t), libc::SIGKILL);
    }
    let output = reader.join().ok()??;
    if !status.success() || output.len() > max_bytes {
        return None;
    }
    let version = sanitize::redact(&String::from_utf8_lossy(&output))
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
pub fn which(binary: &str) -> Option<std::path::PathBuf> {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn which_finds_a_standard_tool_but_not_a_bogus_one() {
        // `sh` exists on every Linux/CI box; a nonsense name does not.
        assert!(which("sh").is_some());
        assert!(which("definitely-not-a-real-binary-xyz").is_none());
    }

    #[test]
    fn detect_named_returns_none_when_version_call_fails() {
        // `false` exists but exits non-zero, so detection must report None
        // rather than a bogus ToolInfo.
        assert!(detect_named("false", &["--version"]).is_none());
    }

    #[test]
    fn detect_named_returns_none_for_missing_binary() {
        assert!(detect_named("definitely-not-a-real-binary-xyz", &["--version"]).is_none());
    }

    #[test]
    fn detect_named_reads_a_version_line() {
        // `env --version` prints a version banner and exits 0 on GNU coreutils.
        if which("env").is_some() {
            if let Some(info) = detect_named("env", &["--version"]) {
                assert_eq!(info.binary, "env");
                assert!(!info.version.is_empty());
                assert!(info.path.ends_with("env"));
            }
        }
    }

    #[test]
    fn detection_caps_version_output() {
        assert!(detect_named_with_limits(
            "head",
            &["-c", "4096", "/dev/zero"],
            1024,
            Duration::from_secs(2),
        )
        .is_none());
    }

    #[test]
    fn detection_times_out() {
        let started = Instant::now();
        assert!(
            detect_named_with_limits("sleep", &["30"], 1024, Duration::from_millis(50),).is_none()
        );
        assert!(started.elapsed() < Duration::from_secs(2));
    }
}
