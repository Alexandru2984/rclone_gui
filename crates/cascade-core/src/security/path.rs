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

/// The spatial relationship between a source and a destination, used to warn
/// about transfers that loop, duplicate, or could delete their own source.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Overlap {
    /// No problematic relationship — safe to proceed.
    None,
    /// Source and destination are the same location.
    Identical,
    /// The destination lives inside the source tree (copying a tree into itself).
    DestInsideSource,
    /// The source lives inside the destination tree; a mirror (`--delete`) here
    /// could remove the destination's other contents.
    SourceInsideDest,
}

impl Overlap {
    /// A human-readable warning, or `None` when there is no overlap.
    pub fn warning(self) -> Option<&'static str> {
        match self {
            Overlap::None => None,
            Overlap::Identical => {
                Some(crate::n("Source and destination are the same location."))
            }
            Overlap::DestInsideSource => Some(crate::n(
                "The destination is inside the source — this can copy a folder into itself.",
            )),
            Overlap::SourceInsideDest => Some(crate::n(
                "The source is inside the destination — a mirror/delete could remove other files there.",
            )),
        }
    }
}

/// Normalize a path for overlap comparison: resolve symlinks/`.`/relative parts
/// via canonicalization where possible (falling back to the parent for a
/// not-yet-existing destination), and strip a trailing slash.
fn normalize_for_overlap(p: &str) -> Option<String> {
    if is_remote_endpoint(p) {
        return Some(p.trim().trim_end_matches('/').to_string());
    }
    let trimmed = p.trim().trim_end_matches('/');
    let trimmed = if trimmed.is_empty() { "/" } else { trimmed };
    let path = std::path::Path::new(trimmed);
    if let Ok(c) = std::fs::canonicalize(path) {
        return Some(c.to_string_lossy().into_owned());
    }
    // The path itself may not exist yet (a fresh destination); resolve its
    // parent so a relative or symlinked destination still compares correctly.
    if let (Some(parent), Some(name)) = (path.parent(), path.file_name()) {
        if let Ok(c) = std::fs::canonicalize(parent) {
            return Some(c.join(name).to_string_lossy().into_owned());
        }
    }
    Some(trimmed.to_string())
}

/// Classify how `source` and `dest` overlap. A local path and a remote endpoint
/// (or two paths on different remotes) can never overlap, so they return
/// [`Overlap::None`]. Comparison is component-aware: `/a/b` is not "inside"
/// `/a/bc`.
pub fn check_overlap(source: &str, dest: &str) -> Overlap {
    // A local path and a remote endpoint cannot share a filesystem location.
    if is_remote_endpoint(source) != is_remote_endpoint(dest) {
        return Overlap::None;
    }
    let (s, d) = match (normalize_for_overlap(source), normalize_for_overlap(dest)) {
        (Some(s), Some(d)) => (s, d),
        _ => return Overlap::None,
    };
    if s == d {
        return Overlap::Identical;
    }
    if d.starts_with(&format!("{s}/")) {
        return Overlap::DestInsideSource;
    }
    if s.starts_with(&format!("{d}/")) {
        return Overlap::SourceInsideDest;
    }
    Overlap::None
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

    #[cfg(unix)]
    #[test]
    fn symlink_to_system_dir_warns_via_canonicalization() {
        // An innocent-looking link that resolves to a system directory must
        // surface the system-dir warning (the canonicalized-verdict path).
        let dir = tempfile::tempdir().unwrap();
        let link = dir.path().join("etc_link");
        std::os::unix::fs::symlink("/etc", &link).unwrap();
        assert!(matches!(
            validate(link.to_str().unwrap()),
            Ok(PathVerdict::Warn(_))
        ));
    }

    #[test]
    fn overlap_identical_paths() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().to_str().unwrap();
        assert_eq!(check_overlap(p, p), Overlap::Identical);
        // A trailing slash must not change the verdict.
        assert_eq!(check_overlap(p, &format!("{p}/")), Overlap::Identical);
    }

    #[test]
    fn overlap_dest_inside_source() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().to_str().unwrap().to_string();
        let dst = dir.path().join("backup");
        std::fs::create_dir_all(&dst).unwrap();
        assert_eq!(
            check_overlap(&src, dst.to_str().unwrap()),
            Overlap::DestInsideSource
        );
    }

    #[test]
    fn overlap_source_inside_dest() {
        let dir = tempfile::tempdir().unwrap();
        let dst = dir.path().to_str().unwrap().to_string();
        let src = dir.path().join("data");
        std::fs::create_dir_all(&src).unwrap();
        assert_eq!(
            check_overlap(src.to_str().unwrap(), &dst),
            Overlap::SourceInsideDest
        );
    }

    #[test]
    fn overlap_sibling_prefix_is_not_overlap() {
        // "/x/b" must not count as inside "/x/bc" — comparison is component-wise.
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("b");
        let b = dir.path().join("bc");
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        assert_eq!(
            check_overlap(a.to_str().unwrap(), b.to_str().unwrap()),
            Overlap::None
        );
    }

    #[test]
    fn overlap_local_vs_remote_is_none() {
        assert_eq!(check_overlap("/home/u/data", "gdrive:data"), Overlap::None);
        assert_eq!(
            check_overlap("gdrive:a", "gdrive:a/b"),
            Overlap::DestInsideSource
        );
        assert_eq!(check_overlap("gdrive:a", "dropbox:a"), Overlap::None);
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

    #[test]
    fn remote_endpoint_edge_cases() {
        // A leading colon is not a remote name (empty name before ':').
        assert!(!is_remote_endpoint(":nope"));
        // No colon at all.
        assert!(!is_remote_endpoint("plainname"));
        assert!(!is_remote_endpoint(""));
        // Multiple colons: the first one (at a valid position) wins.
        assert!(is_remote_endpoint("remote:path:with:colons"));
        // A relative path with a colon in a later component is still local.
        assert!(!is_remote_endpoint("dir/sub:name"));
    }

    #[test]
    fn dotdot_only_matches_whole_component() {
        // A literal ".." component is refused...
        assert!(matches!(
            validate("/a/../b"),
            Err(CoreError::DangerousPath(_))
        ));
        // ...but ".." as part of a longer name is fine (e.g. "..config").
        assert_eq!(validate("/home/tester/..config").unwrap(), PathVerdict::Ok);
        assert_eq!(validate("/data/a..b/c").unwrap(), PathVerdict::Ok);
    }

    #[test]
    fn trailing_and_repeated_slashes_normalize() {
        // Repeated trailing slashes collapse to the bare root and are refused.
        assert!(matches!(validate("////"), Err(CoreError::DangerousPath(_))));
        // A normal path keeps its verdict regardless of a trailing slash.
        assert_eq!(validate("/srv/data///").unwrap(), PathVerdict::Ok);
    }

    #[test]
    fn tabs_and_newlines_are_trimmed_to_empty() {
        assert!(matches!(validate("\t\n  "), Err(CoreError::InvalidPath(_))));
    }

    #[test]
    fn system_dir_prefix_is_not_confused_with_sibling() {
        // "/usrlocal" must NOT be treated as under "/usr".
        assert_eq!(validate("/usrlocal/data").unwrap(), PathVerdict::Ok);
        // But "/usr/local" is under "/usr" and warns.
        assert!(matches!(validate("/usr/local"), Ok(PathVerdict::Warn(_))));
    }

    #[test]
    fn overlap_none_has_no_warning() {
        assert_eq!(Overlap::None.warning(), None);
        assert!(Overlap::Identical.warning().is_some());
        assert!(Overlap::DestInsideSource.warning().is_some());
        assert!(Overlap::SourceInsideDest.warning().is_some());
    }

    #[test]
    fn overlap_is_symmetric_in_shape() {
        // Swapping src/dst must flip DestInsideSource <-> SourceInsideDest.
        let dir = tempfile::tempdir().unwrap();
        let outer = dir.path().to_str().unwrap().to_string();
        let inner = dir.path().join("inner");
        std::fs::create_dir_all(&inner).unwrap();
        let inner = inner.to_str().unwrap();
        assert_eq!(check_overlap(&outer, inner), Overlap::DestInsideSource);
        assert_eq!(check_overlap(inner, &outer), Overlap::SourceInsideDest);
    }

    #[test]
    fn overlap_identical_remotes() {
        assert_eq!(
            check_overlap("gdrive:Photos", "gdrive:Photos"),
            Overlap::Identical
        );
        // Trailing slash on a remote path is normalized away.
        assert_eq!(
            check_overlap("gdrive:Photos/", "gdrive:Photos"),
            Overlap::Identical
        );
    }

    #[test]
    fn overlap_unrelated_paths_are_none() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("alpha");
        let b = dir.path().join("beta");
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        assert_eq!(
            check_overlap(a.to_str().unwrap(), b.to_str().unwrap()),
            Overlap::None
        );
    }

    #[test]
    fn overlap_nonexistent_paths_compare_lexically() {
        // Neither path exists, so canonicalization falls back to the literal
        // form; the check must still catch a nested destination.
        assert_eq!(
            check_overlap("/no/such/root", "/no/such/root/child"),
            Overlap::DestInsideSource
        );
        assert_eq!(
            check_overlap("/no/such/root", "/no/such/other"),
            Overlap::None
        );
    }
}
