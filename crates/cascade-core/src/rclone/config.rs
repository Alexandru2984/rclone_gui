//! Creating and removing rclone remotes from within Cascade.
//!
//! rclone supports dozens of providers; here we expose a curated short list of
//! common ones plus safe argv builders for `rclone config create/delete`.
//! OAuth providers (Drive, Dropbox, OneDrive) open a browser during creation —
//! that is rclone's own flow, which we simply run as a process.

use crate::error::{CoreError, Result};

/// A storage provider Cascade can help configure.
#[derive(Debug, Clone, Copy)]
pub struct Provider {
    /// Friendly label shown in the UI.
    pub label: &'static str,
    /// rclone backend type (the `type` in `rclone config`).
    pub rtype: &'static str,
    /// Whether creating it triggers a browser OAuth sign-in.
    pub oauth: bool,
    /// One-line guidance, e.g. which parameters are needed.
    pub hint: &'static str,
}

/// Curated common providers, in display order.
pub fn providers() -> Vec<Provider> {
    vec![
        Provider {
            label: "Google Drive",
            rtype: "drive",
            oauth: true,
            hint: "Opens a browser to sign in. No parameters needed.",
        },
        Provider {
            label: "Dropbox",
            rtype: "dropbox",
            oauth: true,
            hint: "Opens a browser to sign in. No parameters needed.",
        },
        Provider {
            label: "OneDrive",
            rtype: "onedrive",
            oauth: true,
            hint: "Opens a browser to sign in. No parameters needed.",
        },
        Provider {
            label: "Amazon S3",
            rtype: "s3",
            oauth: false,
            hint: "Params: provider, access_key_id, secret_access_key, region",
        },
        Provider {
            label: "Backblaze B2",
            rtype: "b2",
            oauth: false,
            hint: "Params: account, key",
        },
        Provider {
            label: "SFTP (SSH)",
            rtype: "sftp",
            oauth: false,
            hint: "Params: host, user, and pass or key_file",
        },
        Provider {
            label: "WebDAV",
            rtype: "webdav",
            oauth: false,
            hint: "Params: url, vendor, user, pass",
        },
        Provider {
            label: "FTP",
            rtype: "ftp",
            oauth: false,
            hint: "Params: host, user, pass",
        },
        Provider {
            label: "Local disk",
            rtype: "local",
            oauth: false,
            hint: "No parameters needed.",
        },
    ]
}

/// Validate an rclone remote name: non-empty, only letters/digits/_/-/space,
/// and free of the `:` and `/` that have special meaning in rclone paths.
pub fn validate_remote_name(name: &str) -> Result<()> {
    let n = name.trim();
    if n.is_empty() {
        return Err(CoreError::InvalidCommand("remote name is empty".into()));
    }
    if n.chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        Ok(())
    } else {
        Err(CoreError::InvalidCommand(
            "remote name may only contain letters, digits, '_' and '-'".into(),
        ))
    }
}

/// argv for `rclone config create <name> <type> [key value ...] --obscure`.
///
/// `--obscure` makes rclone obscure password-type values automatically, so the
/// caller may pass plaintext passwords. Each (key, value) becomes two argv items.
pub fn config_create_args(
    name: &str,
    rtype: &str,
    params: &[(String, String)],
) -> Result<Vec<String>> {
    validate_remote_name(name)?;
    if rtype.trim().is_empty() {
        return Err(CoreError::InvalidCommand("provider type is empty".into()));
    }
    let mut args = vec![
        "config".to_string(),
        "create".to_string(),
        name.to_string(),
        rtype.to_string(),
    ];
    for (k, v) in params {
        args.push(k.clone());
        args.push(v.clone());
    }
    args.push("--obscure".into());
    Ok(args)
}

/// argv for `rclone config delete <name>`.
pub fn config_delete_args(name: &str) -> Result<Vec<String>> {
    validate_remote_name(name)?;
    Ok(vec![
        "config".to_string(),
        "delete".to_string(),
        name.to_string(),
    ])
}

/// Parse a "key=value key2=value2" parameters string into pairs.
pub fn parse_params(input: &str) -> Result<Vec<(String, String)>> {
    let tokens = crate::security::flags::parse(input)?;
    let mut pairs = Vec::new();
    for t in tokens {
        match t.split_once('=') {
            Some((k, v)) if !k.is_empty() => pairs.push((k.to_string(), v.to_string())),
            _ => {
                return Err(CoreError::InvalidCommand(format!(
                    "parameter '{t}' must be in key=value form"
                )))
            }
        }
    }
    Ok(pairs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn providers_include_common_clouds() {
        let types: Vec<&str> = providers().iter().map(|p| p.rtype).collect();
        for t in ["drive", "dropbox", "onedrive", "s3", "sftp"] {
            assert!(types.contains(&t), "missing provider {t}");
        }
    }

    #[test]
    fn name_validation() {
        assert!(validate_remote_name("gdrive").is_ok());
        assert!(validate_remote_name("my-remote_2").is_ok());
        assert!(validate_remote_name("").is_err());
        assert!(validate_remote_name("bad:name").is_err());
        assert!(validate_remote_name("bad/name").is_err());
    }

    #[test]
    fn create_args_layout() {
        let args = config_create_args(
            "box",
            "sftp",
            &[
                ("host".into(), "example.com".into()),
                ("user".into(), "bob".into()),
            ],
        )
        .unwrap();
        assert_eq!(&args[..4], &["config", "create", "box", "sftp"]);
        assert!(args.windows(2).any(|w| w == ["host", "example.com"]));
        assert!(args.windows(2).any(|w| w == ["user", "bob"]));
        assert_eq!(args.last().unwrap(), "--obscure");
    }

    #[test]
    fn create_args_reject_bad_name() {
        assert!(config_create_args("a:b", "drive", &[]).is_err());
    }

    #[test]
    fn delete_args_layout() {
        assert_eq!(
            config_delete_args("box").unwrap(),
            vec!["config", "delete", "box"]
        );
    }

    #[test]
    fn params_parse_into_pairs() {
        let pairs = parse_params("host=example.com user=bob").unwrap();
        assert_eq!(
            pairs,
            vec![
                ("host".into(), "example.com".into()),
                ("user".into(), "bob".into())
            ]
        );
        assert!(parse_params("nokeyvalue").is_err());
        assert!(parse_params("").unwrap().is_empty());
    }
}
