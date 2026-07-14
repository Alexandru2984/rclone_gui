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
            label: crate::n("Google Drive"),
            rtype: "drive",
            oauth: true,
            hint: crate::n("Opens a browser to sign in. No parameters needed."),
        },
        Provider {
            label: crate::n("Dropbox"),
            rtype: "dropbox",
            oauth: true,
            hint: crate::n("Opens a browser to sign in. No parameters needed."),
        },
        Provider {
            label: crate::n("OneDrive"),
            rtype: "onedrive",
            oauth: true,
            hint: crate::n("Opens a browser to sign in. No parameters needed."),
        },
        Provider {
            label: crate::n("Amazon S3"),
            rtype: "s3",
            oauth: false,
            hint: crate::n(
                "Non-secret params: provider, env_auth, region. Configure inline keys with rclone config in a terminal.",
            ),
        },
        Provider {
            label: crate::n("Backblaze B2"),
            rtype: "b2",
            oauth: false,
            hint: crate::n("Credential setup requires rclone config in a terminal."),
        },
        Provider {
            label: crate::n("SFTP (SSH)"),
            rtype: "sftp",
            oauth: false,
            hint: crate::n(
                "Non-secret params: host, user, key_file. Configure passwords in a terminal.",
            ),
        },
        Provider {
            label: crate::n("WebDAV"),
            rtype: "webdav",
            oauth: false,
            hint: crate::n(
                "Non-secret params: url, vendor, user. Configure passwords in a terminal.",
            ),
        },
        Provider {
            label: crate::n("FTP"),
            rtype: "ftp",
            oauth: false,
            hint: crate::n("Non-secret params: host, user. Configure passwords in a terminal."),
        },
        Provider {
            label: crate::n("Local disk"),
            rtype: "local",
            oauth: false,
            hint: crate::n("No parameters needed."),
        },
    ]
}

/// Validate an rclone remote name: non-empty, only letters, digits, `_` and
/// `-`, and free of the `:` and `/` that have special meaning in rclone paths.
pub fn validate_remote_name(name: &str) -> Result<()> {
    let n = name.trim();
    if n.is_empty() {
        return Err(CoreError::InvalidCommand("remote name is empty".into()));
    }
    if n.len() <= 128
        && n.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        && n == name
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
    if params_contain_secret(params) {
        return Err(CoreError::InvalidCommand(
            "credential-bearing remote parameters are blocked because rclone config create would expose them in the process list; use OAuth or run rclone config in a terminal"
                .into(),
        ));
    }
    let mut args = vec![
        "config".to_string(),
        "create".to_string(),
        "--obscure".to_string(),
        "--".to_string(),
        name.to_string(),
        rtype.to_string(),
    ];
    for (k, v) in params {
        if k.is_empty()
            || k.len() > 256
            || v.len() > 4096
            || k.chars().any(char::is_control)
            || v.chars().any(char::is_control)
        {
            return Err(CoreError::InvalidCommand(
                "remote parameters exceed safety limits or contain control characters".into(),
            ));
        }
        args.push(k.clone());
        args.push(v.clone());
    }
    Ok(args)
}

/// argv for `rclone config delete <name>`.
pub fn config_delete_args(name: &str) -> Result<Vec<String>> {
    validate_remote_name(name)?;
    Ok(vec![
        "config".to_string(),
        "delete".to_string(),
        "--".to_string(),
        name.to_string(),
    ])
}

/// Whether a parameter set carries a credential (password/secret/token/key).
///
/// Used by the core boundary and GUI to reject credentials before
/// `rclone config create` can place them in its process argv.
pub fn params_contain_secret(params: &[(String, String)]) -> bool {
    params.iter().any(|(key, value)| {
        let key = key.to_ascii_lowercase().replace('-', "_");
        let secret_key = key.contains("pass")
            || key.contains("secret")
            || key.contains("token")
            || key.contains("credential")
            || key.contains("access_key")
            || key.contains("api_key")
            || key.contains("private_key")
            || key.contains("key_pem")
            || key == "key"
            || key.ends_with("_key");
        secret_key || crate::security::sanitize::contains_secret(&format!("{key}={value}"))
    })
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
        assert_eq!(
            &args[..6],
            &["config", "create", "--obscure", "--", "box", "sftp"]
        );
        assert!(args.windows(2).any(|w| w == ["host", "example.com"]));
        assert!(args.windows(2).any(|w| w == ["user", "bob"]));
        assert_eq!(args.last().unwrap(), "bob");
    }

    #[test]
    fn create_args_reject_bad_name() {
        assert!(config_create_args("a:b", "drive", &[]).is_err());
    }

    #[test]
    fn delete_args_layout() {
        assert_eq!(
            config_delete_args("box").unwrap(),
            vec!["config", "delete", "--", "box"]
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

    #[test]
    fn remote_name_rejects_spaces_and_symbols() {
        // Matches the code (spaces are NOT allowed, despite older docs).
        assert!(validate_remote_name("my remote").is_err());
        assert!(validate_remote_name("weird!name").is_err());
        assert!(validate_remote_name("   ").is_err());
        // Surrounding whitespace is rejected so validation and argv agree.
        assert!(validate_remote_name("  gdrive  ").is_err());
    }

    #[test]
    fn params_split_on_first_equals_only() {
        // A value may itself contain '=' (e.g. a base64 token).
        let pairs = parse_params("key=a=b=c").unwrap();
        assert_eq!(pairs, vec![("key".into(), "a=b=c".into())]);
    }

    #[test]
    fn params_allow_empty_value_but_not_empty_key() {
        // "k=" is a valid (empty) value.
        assert_eq!(parse_params("k=").unwrap(), vec![("k".into(), "".into())]);
        // "=v" has no key and is refused.
        assert!(parse_params("=v").is_err());
    }

    #[test]
    fn params_honor_quoting() {
        let pairs = parse_params(r#"pass="a b c" host=example.com"#).unwrap();
        assert_eq!(pairs[0], ("pass".into(), "a b c".into()));
        assert_eq!(pairs[1], ("host".into(), "example.com".into()));
    }

    #[test]
    fn create_args_reject_empty_type() {
        assert!(config_create_args("box", "  ", &[]).is_err());
    }

    #[test]
    fn create_args_refuse_credentials_at_the_process_boundary() {
        for params in [
            vec![("pass".into(), "hunter2".into())],
            vec![("secret_access_key".into(), "aws-secret".into())],
            vec![(
                "url".into(),
                "https://alice:password@example.com/data".into(),
            )],
        ] {
            assert!(config_create_args("box", "s3", &params).is_err());
        }
    }

    #[test]
    fn create_args_without_params_still_obscure() {
        let args = config_create_args("box", "local", &[]).unwrap();
        assert_eq!(
            args,
            vec!["config", "create", "--obscure", "--", "box", "local"]
        );
    }

    #[test]
    fn secret_params_are_detected() {
        let mk = |k: &str| vec![(k.to_string(), "v".to_string())];
        for k in [
            "pass",
            "password",
            "sftp-pass",
            "secret_access_key",
            "client_secret",
            "token",
            "key",
            "key_pem",
            "PASSWORD", // case-insensitive
        ] {
            assert!(params_contain_secret(&mk(k)), "{k} should warn");
        }
        for k in [
            "host", "user", "region", "url", "vendor", "provider", "key_file",
        ] {
            assert!(!params_contain_secret(&mk(k)), "{k} should not warn");
        }
        assert!(params_contain_secret(&[(
            "url".into(),
            "https://alice:password@example.com".into()
        )]));
        assert!(!params_contain_secret(&[]));
    }

    #[test]
    fn every_provider_has_nonempty_label_and_type() {
        for p in providers() {
            assert!(!p.label.is_empty());
            assert!(!p.rtype.is_empty());
            assert!(!p.hint.is_empty());
        }
    }
}
