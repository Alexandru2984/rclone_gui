//! Listing rclone remotes and browsing their contents via `lsjson`.
//!
//! These functions only *build argv* and *parse output*; running the command
//! (off the UI thread) is the caller's job via [`crate::process::capture`].
//! Keeping parse logic pure makes it unit-testable without rclone installed.

use serde::Deserialize;

use crate::error::Result;

/// One filesystem entry returned by `rclone lsjson`.
#[derive(Debug, Clone, Deserialize)]
pub struct Entry {
    #[serde(rename = "Name")]
    pub name: String,
    #[serde(rename = "Path")]
    pub path: String,
    #[serde(rename = "Size", default)]
    pub size: i64,
    #[serde(rename = "IsDir", default)]
    pub is_dir: bool,
}

/// argv for `rclone listremotes`.
pub fn listremotes_args() -> Vec<String> {
    vec!["listremotes".into()]
}

/// argv for `rclone lsjson <path>`. `path` is passed as a single argv item.
pub fn lsjson_args(path: &str) -> Vec<String> {
    vec!["lsjson".into(), path.to_string()]
}

/// Parse `listremotes` output into remote names (each keeps its trailing `:`).
pub fn parse_remotes(stdout: &str) -> Vec<String> {
    stdout
        .lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty())
        .map(|l| l.to_string())
        .collect()
}

/// Parse `lsjson` output, returning entries sorted directories-first then by name.
pub fn parse_lsjson(stdout: &str) -> Result<Vec<Entry>> {
    let mut entries: Vec<Entry> = serde_json::from_str(stdout)?;
    entries.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    Ok(entries)
}

/// Join a remote (`gdrive:`) and a sub-path (`Photos/2024`) into an rclone path.
pub fn join(remote: &str, sub: &str) -> String {
    let sub = sub.trim_matches('/');
    if sub.is_empty() {
        remote.to_string()
    } else {
        format!("{remote}{sub}")
    }
}

/// Drop the last path component of a sub-path (for "go up"). Returns "" at root.
pub fn parent_sub(sub: &str) -> String {
    let trimmed = sub.trim_matches('/');
    match trimmed.rsplit_once('/') {
        Some((head, _)) => head.to_string(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_remotes() {
        let out = "gdrive:\nonedrive:\n\n  sftp:  \n";
        assert_eq!(parse_remotes(out), vec!["gdrive:", "onedrive:", "sftp:"]);
    }

    #[test]
    fn parses_and_sorts_lsjson() {
        let json = r#"[
            {"Name":"zeta.txt","Path":"zeta.txt","Size":10,"IsDir":false},
            {"Name":"Alpha","Path":"Alpha","Size":-1,"IsDir":true},
            {"Name":"beta.txt","Path":"beta.txt","Size":20,"IsDir":false}
        ]"#;
        let entries = parse_lsjson(json).unwrap();
        // Directories first, then case-insensitive by name.
        assert_eq!(entries[0].name, "Alpha");
        assert!(entries[0].is_dir);
        assert_eq!(entries[1].name, "beta.txt");
        assert_eq!(entries[2].name, "zeta.txt");
    }

    #[test]
    fn join_and_parent() {
        assert_eq!(join("gdrive:", ""), "gdrive:");
        assert_eq!(join("gdrive:", "Photos/2024"), "gdrive:Photos/2024");
        assert_eq!(join("gdrive:", "/Photos/"), "gdrive:Photos");
        assert_eq!(parent_sub("Photos/2024"), "Photos");
        assert_eq!(parent_sub("Photos"), "");
        assert_eq!(parent_sub(""), "");
    }

    #[test]
    fn lsjson_argv_is_two_items() {
        assert_eq!(
            lsjson_args("gdrive:Photos"),
            vec!["lsjson", "gdrive:Photos"]
        );
    }
}
