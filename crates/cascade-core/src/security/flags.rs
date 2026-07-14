//! Parsing user-supplied custom flags into individual argv tokens.
//!
//! Cascade never runs a shell, so the danger is not shell injection but
//! malformed tokens. We use `shlex` for correct POSIX quoting/escaping, then
//! reject NUL bytes (which cannot appear in an argv item anyway).

use crate::error::{CoreError, Result};
use crate::security::sanitize;
use crate::Tool;

/// Split a custom-flags string into argv tokens using POSIX shell rules
/// (single/double quotes and backslash escapes), via the `shlex` crate.
pub fn parse(input: &str) -> Result<Vec<String>> {
    if input.contains('\0') {
        return Err(CoreError::InvalidCommand(
            "custom flags contain a NUL byte".into(),
        ));
    }
    match shlex::split(input) {
        Some(tokens) => Ok(tokens),
        None => Err(CoreError::InvalidCommand(
            "could not parse custom flags (check quotes/escapes)".into(),
        )),
    }
}

/// Validate power-user flags without allowing them to become extra operands,
/// override application safety controls, or select an executable transport.
///
/// Custom options must use their long form. Options with values use
/// `--option=value`, which keeps every token self-contained and prevents a bare
/// value from being interpreted as an additional source/destination operand.
pub fn validate_extra(tool: Tool, tokens: &[String]) -> Result<()> {
    for token in tokens {
        if token == "--" || !token.starts_with("--") || token.len() <= 2 {
            return Err(CoreError::InvalidCommand(format!(
                "custom flag '{token}' must use long form (--option or --option=value)"
            )));
        }

        if sanitize::contains_secret(token) {
            return Err(CoreError::InvalidCommand(
                "custom flags must not contain credentials; configure authentication outside +                 Cascade and reference it without embedding the secret"
                    .into(),
            ));
        }

        let key = token[2..]
            .split_once('=')
            .map_or(&token[2..], |(key, _)| key)
            .to_ascii_lowercase();
        let effective = key.strip_prefix("no-").unwrap_or(&key);

        let common_controlled = [
            "dry-run",
            "max-delete",
            "backup-dir",
            "include",
            "exclude",
            "checksum",
        ];
        let tool_controlled: &[&str] = match tool {
            Tool::Rclone => &[
                "transfers",
                "checkers",
                "bwlimit",
                "retries",
                "stats",
                "stats-one-line",
                "stats-log-level",
                "resync",
            ],
            Tool::Rsync => &[
                "archive",
                "compress",
                "rsh",
                "rsync-path",
                "remote-option",
                "old-args",
                "secluded-args",
                "protect-args",
                "itemize-changes",
                "info",
                "outbuf",
            ],
        };

        let changes_operation = effective.starts_with("delete")
            || effective.starts_with("remove-source")
            || effective == "remove-sent-files"
            || effective == "password-command";
        if common_controlled.contains(&effective)
            || tool_controlled.contains(&effective)
            || changes_operation
        {
            return Err(CoreError::InvalidCommand(format!(
                "custom flag '--{key}' is controlled by Cascade for safety"
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_is_no_tokens() {
        assert_eq!(parse("").unwrap(), Vec::<String>::new());
        assert_eq!(parse("   ").unwrap(), Vec::<String>::new());
    }

    #[test]
    fn splits_on_whitespace() {
        assert_eq!(
            parse("--fast-list --checksum").unwrap(),
            vec!["--fast-list", "--checksum"]
        );
    }

    #[test]
    fn honors_quotes_with_spaces() {
        assert_eq!(
            parse("--exclude '*.tmp file' --x \"a b\"").unwrap(),
            vec!["--exclude", "*.tmp file", "--x", "a b"]
        );
    }

    #[test]
    fn honors_backslash_escapes() {
        // shlex understands escaped quotes inside double quotes.
        assert_eq!(
            parse(r#"--exclude "a\"b""#).unwrap(),
            vec!["--exclude", "a\"b"]
        );
        assert_eq!(parse(r"a\ b").unwrap(), vec!["a b"]);
    }

    #[test]
    fn unclosed_quote_is_error() {
        assert!(parse("--exclude 'oops").is_err());
    }

    #[test]
    fn nul_byte_rejected() {
        assert!(parse("--flag\0bad").is_err());
    }

    #[test]
    fn collapses_runs_of_whitespace() {
        assert_eq!(
            parse("--a    --b\t\t--c").unwrap(),
            vec!["--a", "--b", "--c"]
        );
    }

    #[test]
    fn keeps_equals_and_braces_as_one_token() {
        assert_eq!(
            parse("--exclude={*.tmp,*.log}").unwrap(),
            vec!["--exclude={*.tmp,*.log}"]
        );
    }

    #[test]
    fn preserves_unicode_values() {
        assert_eq!(
            parse("--dest 'Café/Ünïcode'").unwrap(),
            vec!["--dest", "Café/Ünïcode"]
        );
    }

    #[test]
    fn leading_and_trailing_whitespace_ignored() {
        assert_eq!(
            parse("   --flag value   ").unwrap(),
            vec!["--flag", "value"]
        );
    }

    #[test]
    fn mismatched_quote_kinds_are_ok() {
        // A single quote inside double quotes is a literal.
        assert_eq!(
            parse(r#"--x "it's fine""#).unwrap(),
            vec!["--x", "it's fine"]
        );
    }

    #[test]
    fn extra_flags_require_self_contained_long_options() {
        assert!(validate_extra(Tool::Rclone, &["--fast-list".into()]).is_ok());
        assert!(validate_extra(Tool::Rclone, &["--metadata-set=x=y".into()]).is_ok());
        for bad in ["-n", "--", "value", "--bwlimit", "10M"] {
            assert!(
                validate_extra(Tool::Rclone, &[bad.into()]).is_err(),
                "{bad}"
            );
        }
    }

    #[test]
    fn extra_flags_cannot_override_safety_controls() {
        for bad in [
            "--dry-run=false",
            "--no-dry-run",
            "--max-delete=-1",
            "--delete-before",
            "--backup-dir=/tmp/x",
            "--password-command=/bin/evil",
        ] {
            assert!(
                validate_extra(Tool::Rclone, &[bad.into()]).is_err(),
                "{bad}"
            );
        }
        for bad in [
            "--rsh=/bin/evil",
            "--rsync-path=/bin/evil",
            "--old-args",
            "--remote-option=--delete",
        ] {
            assert!(validate_extra(Tool::Rsync, &[bad.into()]).is_err(), "{bad}");
        }
    }

    #[test]
    fn extra_flags_cannot_embed_credentials() {
        for bad in [
            "--s3-secret-access-key=aws-secret",
            "--crypt-password=crypt-secret",
            "--header=Authorization: Bearer token-secret",
        ] {
            assert!(
                validate_extra(Tool::Rclone, &[bad.into()]).is_err(),
                "{bad}"
            );
        }
    }
}
