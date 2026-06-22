//! Path validation — the first line of defence against catastrophic mistakes.
//!
//! These checks apply to **local** paths. Remote endpoints (e.g. `gdrive:photos`)
//! are validated separately by the rclone layer.

use crate::error::{CoreError, Result};

/// Outcome of validating a single local path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PathVerdict {
    /// Safe to use as-is.
    Ok,
    /// Usable, but the user should be warned (e.g. a system directory).
    Warn(String),
}

/// Returns `true` if `s` looks like an rclone remote endpoint (`remote:path`)
/// rather than a local filesystem path. We do not apply local-path rules to it.
pub fn is_remote_endpoint(s: &str) -> bool {
    // A remote is `name:` or `name:path`, where name has no slash and is non-empty.
    // Guard against Windows drive letters is unnecessary on Linux.
    match s.find(':') {
        Some(idx) if idx > 0 => !s[..idx].contains('/'),
        _ => false,
    }
}

/// Validate a local path intended as a source or destination.
///
/// Rejects empty/whitespace paths, paths containing `..`, the filesystem root
/// `/`, and a bare `$HOME`. Warns (but allows) well-known system directories.
/// If the path exists, it is canonicalized first so that **symlinks pointing at
/// `/` or `$HOME` cannot slip past the guard**.
pub fn validate(raw: &str) -> Result<PathVerdict> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(CoreError::InvalidPath("path is empty".into()));
    }

    // Normalize a trailing slash away (except for the bare root) for comparison.
    let stripped = trimmed.trim_end_matches('/');
    let normalized = if stripped.is_empty() { "/" } else { stripped };

    // `..` could resolve to a forbidden location; require explicit paths.
    if normalized.split('/').any(|c| c == "..") {
        return Err(CoreError::DangerousPath(
            "path must not contain '..'; use an explicit path".into(),
        ));
    }

    // Check the literal path, then (if it exists) its symlink-resolved form.
    let verdict = classify_dangerous(normalized)?;
    if let Ok(canon) = std::fs::canonicalize(normalized) {
        let canon = canon.to_string_lossy();
        let canon_verdict = classify_dangerous(&canon)?; // may reject root/home
        if matches!(canon_verdict, PathVerdict::Warn(_)) {
            return Ok(canon_verdict);
        }
    }
    Ok(verdict)
}

/// The dangerous-location checks, applied to an already-normalized path.
fn classify_dangerous(normalized: &str) -> Result<PathVerdict> {
    if normalized == "/" {
        return Err(CoreError::DangerousPath(
            "the filesystem root '/' cannot be a source or destination".into(),
        ));
    }

    if let Ok(home) = std::env::var("HOME") {
        let home_norm = home.trim_end_matches('/');
        if !home_norm.is_empty() && normalized == home_norm {
            return Err(CoreError::DangerousPath(
                "the entire home directory is refused as a target; pick a subfolder".into(),
            ));
        }
    }

    const SYSTEM_DIRS: &[&str] = &[
        "/etc", "/usr", "/bin", "/sbin", "/lib", "/lib64", "/boot", "/proc", "/sys", "/dev", "/var",
    ];
    for sys in SYSTEM_DIRS {
        if normalized == *sys || normalized.starts_with(&format!("{sys}/")) {
            return Ok(PathVerdict::Warn(format!(
                "'{normalized}' is a system directory — proceed only if you are certain"
            )));
        }
    }

    Ok(PathVerdict::Ok)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_is_rejected() {
        assert!(matches!(validate("   "), Err(CoreError::InvalidPath(_))));
        assert!(matches!(validate(""), Err(CoreError::InvalidPath(_))));
    }

    #[test]
    fn root_is_rejected() {
        assert!(matches!(validate("/"), Err(CoreError::DangerousPath(_))));
        assert!(matches!(validate("///"), Err(CoreError::DangerousPath(_))));
    }

    #[test]
    fn bare_home_is_rejected() {
        std::env::set_var("HOME", "/home/tester");
        assert!(matches!(
            validate("/home/tester"),
            Err(CoreError::DangerousPath(_))
        ));
        assert!(matches!(
            validate("/home/tester/"),
            Err(CoreError::DangerousPath(_))
        ));
        // A subfolder of home is fine.
        assert_eq!(validate("/home/tester/Pictures").unwrap(), PathVerdict::Ok);
    }

    #[test]
    fn system_dirs_warn_but_allow() {
        assert!(matches!(validate("/etc"), Ok(PathVerdict::Warn(_))));
        assert!(matches!(validate("/usr/local"), Ok(PathVerdict::Warn(_))));
    }

    #[test]
    fn normal_path_is_ok() {
        assert_eq!(validate("/home/tester/projects").unwrap(), PathVerdict::Ok);
        assert_eq!(validate("/mnt/backup/").unwrap(), PathVerdict::Ok);
    }

    #[test]
    fn rejects_dotdot_components() {
        assert!(matches!(
            validate("/home/u/../.."),
            Err(CoreError::DangerousPath(_))
        ));
        assert!(matches!(
            validate("/srv/data/../../.."),
            Err(CoreError::DangerousPath(_))
        ));
    }

    #[cfg(unix)]
    #[test]
    fn symlink_to_root_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let link = dir.path().join("root_link");
        std::os::unix::fs::symlink("/", &link).unwrap();
        // The literal path looks innocent, but it resolves to "/".
        assert!(matches!(
            validate(link.to_str().unwrap()),
            Err(CoreError::DangerousPath(_))
        ));
    }

    #[test]
    fn detects_remote_endpoints() {
        assert!(is_remote_endpoint("gdrive:"));
        assert!(is_remote_endpoint("gdrive:Photos/2024"));
        assert!(is_remote_endpoint("onedrive:backup"));
        assert!(!is_remote_endpoint("/home/tester"));
        assert!(!is_remote_endpoint("./relative/path"));
        assert!(!is_remote_endpoint("/has/colon:in/path"));
    }
}
