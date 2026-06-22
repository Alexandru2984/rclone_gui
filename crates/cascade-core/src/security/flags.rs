//! Parsing user-supplied custom flags into individual argv tokens.
//!
//! Cascade never runs a shell, so the danger is not shell injection but
//! malformed/garbage tokens. We do a small quote-aware split (supporting
//! `'single'` and `"double"` quotes) and reject control characters. The result
//! is a `Vec<String>` where each element becomes exactly one argv item.

use crate::error::{CoreError, Result};

/// Split a custom-flags string into argv tokens.
///
/// - Whitespace separates tokens.
/// - Single and double quotes group a token and may contain spaces.
/// - Control characters (including newlines and NUL) are rejected.
pub fn parse(input: &str) -> Result<Vec<String>> {
    if input.chars().any(|c| c.is_control()) {
        return Err(CoreError::InvalidCommand(
            "custom flags contain control characters".into(),
        ));
    }

    let mut tokens = Vec::new();
    let mut cur = String::new();
    let mut in_token = false;
    let mut quote: Option<char> = None;

    for c in input.chars() {
        match quote {
            Some(q) => {
                if c == q {
                    quote = None;
                } else {
                    cur.push(c);
                }
            }
            None => match c {
                '\'' | '"' => {
                    quote = Some(c);
                    in_token = true;
                }
                c if c.is_whitespace() => {
                    if in_token {
                        tokens.push(std::mem::take(&mut cur));
                        in_token = false;
                    }
                }
                c => {
                    cur.push(c);
                    in_token = true;
                }
            },
        }
    }

    if quote.is_some() {
        return Err(CoreError::InvalidCommand(
            "custom flags have an unclosed quote".into(),
        ));
    }
    if in_token {
        tokens.push(cur);
    }
    Ok(tokens)
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
    fn unclosed_quote_is_error() {
        assert!(parse("--exclude 'oops").is_err());
    }

    #[test]
    fn control_chars_rejected() {
        assert!(parse("--flag\nrm -rf").is_err());
    }
}
