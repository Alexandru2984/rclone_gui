//! Parsing user-supplied custom flags into individual argv tokens.
//!
//! Cascade never runs a shell, so the danger is not shell injection but
//! malformed tokens. We use `shlex` for correct POSIX quoting/escaping, then
//! reject NUL bytes (which cannot appear in an argv item anyway).

use crate::error::{CoreError, Result};

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
}
