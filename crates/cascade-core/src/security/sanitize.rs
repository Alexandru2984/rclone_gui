//! Log sanitization — redact secrets before any line is shown or written to disk.
//!
//! This runs on **every** stdout/stderr line. It is intentionally conservative:
//! false positives (over-redaction) are acceptable; leaking a token is not.

use std::sync::OnceLock;

use regex::Regex;

const REDACTED: &str = "«redacted»";

struct Patterns {
    rules: Vec<(Regex, &'static str)>,
    secret_option: Regex,
}

fn patterns() -> &'static Patterns {
    static P: OnceLock<Patterns> = OnceLock::new();
    P.get_or_init(|| {
        let rules = vec![
            // credentials embedded in a URL: scheme://user:pass@host
            (
                Regex::new(r"(?i)([a-z][a-z0-9+.\-]*://[^\s:/@]+:)[^\s@]+(@)").unwrap(),
                "$1«redacted»$2",
            ),
            // common secret-bearing CLI flags: --pass X, --password=X, --rc-pass, --token …
            (
                Regex::new(
                    r#"(?i)(--(?:rc-user|[a-z0-9-]*(?:pass(?:word)?|secret|token|credential|api-key|access-key|private-key|key-pem)[a-z0-9-]*)[= ])(?:"[^"]*"|'[^']*'|\S+)"#,
                )
                .unwrap(),
                "$1«redacted»",
            ),
            // OAuth/JSON token blobs: "token":{...} or token: {...}
            (
                Regex::new(r#"(?i)("?token"?\s*[:=]\s*)\{[^}]*\}"#).unwrap(),
                "$1«redacted»",
            ),
            // bearer / Authorization headers
            (
                Regex::new(r"(?i)(authorization:\s*bearer\s+)\S+").unwrap(),
                "$1«redacted»",
            ),
            // access/refresh token key-values
            (
                Regex::new(r#"(?i)((?:access|refresh)_token"?\s*[:=]\s*"?)[A-Za-z0-9._\-]+"#).unwrap(),
                "$1«redacted»",
            ),
            // Generic `key=value` config entries such as pass=,
            // s3_secret_access_key=, api-token=, or private_key=.
            (
                Regex::new(
                    r#"(?i)([a-z0-9_-]*(?:pass(?:word)?|secret|token|credential|api[_-]?key|access[_-]?key|private[_-]?key|key[_-]?pem)[a-z0-9_-]*\s*=\s*["']?)([^"',}\s]+)"#,
                )
                .unwrap(),
                "$1«redacted»",
            ),
            // JSON object `key: value` forms. Requiring an object delimiter
            // avoids treating an rclone remote such as `token:path` as a
            // credential.
            (
                Regex::new(
                    r#"(?i)([,{]\s*["']?[a-z0-9_-]*(?:pass(?:word)?|secret|token|credential|api[_-]?key|access[_-]?key|private[_-]?key|key[_-]?pem)[a-z0-9_-]*["']?\s*:\s*["']?)([^"',}\s]+)"#,
                )
                .unwrap(),
                "$1«redacted»",
            ),
            // Line-oriented config `key: value` forms require whitespace after
            // the colon for the same remote-name ambiguity reason.
            (
                Regex::new(
                    r#"(?im)^(\s*["']?[a-z0-9_-]*(?:pass(?:word)?|secret|token|credential|api[_-]?key|access[_-]?key|private[_-]?key|key[_-]?pem)[a-z0-9_-]*["']?\s*:\s+["']?)([^"',}\s]+)"#,
                )
                .unwrap(),
                "$1«redacted»",
            ),
            // PEM private-key bodies
            (
                Regex::new(
                    r"(?s)-----BEGIN [^-]*PRIVATE KEY-----.*?-----END [^-]*PRIVATE KEY-----",
                )
                .unwrap(),
                REDACTED,
            ),
        ];
        let secret_option = Regex::new(
            r"(?i)--(?:rc-user|[a-z0-9-]*(?:pass(?:word)?|secret|token|credential|api-key|access-key|private-key|key-pem)[a-z0-9-]*)",
        )
        .unwrap();
        Patterns {
            rules,
            secret_option,
        }
    })
}

/// Redact secrets from a single log line (or multi-line chunk).
pub fn redact(input: &str) -> String {
    let mut out = input.to_string();
    for (re, replacement) in &patterns().rules {
        out = re.replace_all(&out, *replacement).into_owned();
    }
    out
}

/// Conservatively detect secret-bearing text, including a legacy split-form
/// option such as `--sftp-pass` whose value lives in the next argv token.
pub fn contains_secret(input: &str) -> bool {
    patterns().secret_option.is_match(input) || redact(input) != input
}

/// Stateful redactor for line-oriented process output.
///
/// A PEM private key spans multiple lines, so applying [`redact`] to each line
/// independently is not sufficient. Once a private-key header is observed,
/// every subsequent line is suppressed through the matching footer.
#[derive(Debug, Default)]
pub struct StreamRedactor {
    inside_private_key: bool,
}

impl StreamRedactor {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Redact one logical line. `None` means that the line was part of a PEM
    /// private-key body and must not be emitted at all.
    pub fn redact_line(&mut self, line: &str) -> Option<String> {
        if self.inside_private_key {
            if is_private_key_end(line) {
                self.inside_private_key = false;
            }
            return None;
        }

        if is_private_key_begin(line) {
            if !is_private_key_end(line) {
                self.inside_private_key = true;
            }
            return Some(REDACTED.to_string());
        }

        Some(redact(line))
    }
}

fn is_private_key_begin(line: &str) -> bool {
    line.contains("-----BEGIN ") && line.contains("PRIVATE KEY-----")
}

fn is_private_key_end(line: &str) -> bool {
    line.contains("-----END ") && line.contains("PRIVATE KEY-----")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_url_credentials() {
        let line = "connecting to sftp://alice:s3cr3tP@ss@example.com:22";
        let out = redact(line);
        assert!(!out.contains("s3cr3tP@ss"));
        assert!(out.contains("alice:«redacted»@"));
    }

    #[test]
    fn redacts_password_flags() {
        assert!(!redact("rclone --password hunter2 foo").contains("hunter2"));
        assert!(!redact("--rc-pass=topsecret").contains("topsecret"));
        assert!(!redact("--token abc.def.ghi").contains("abc.def.ghi"));
    }

    #[test]
    fn redacts_provider_specific_secret_flags() {
        for flag in [
            "--s3-secret-access-key=aws-secret",
            "--crypt-password crypt-secret",
            "--webdav-pass=webdav-secret",
            "--api-token token-secret",
            "--sftp-key-pem=private-material",
        ] {
            let out = redact(flag);
            assert!(!out.contains(flag.split(['=', ' ']).next_back().unwrap()));
        }
    }

    #[test]
    fn redacts_generic_config_and_json_secrets() {
        for line in [
            "pass=inline-secret",
            "s3_secret_access_key=aws-secret",
            r#"{"client_secret":"oauth-secret"}"#,
            r#"{"api_token":"api-secret"}"#,
        ] {
            let out = redact(line);
            assert!(!out.contains("inline-secret"));
            assert!(!out.contains("aws-secret"));
            assert!(!out.contains("oauth-secret"));
            assert!(!out.contains("api-secret"));
        }
    }

    #[test]
    fn remote_names_that_resemble_secret_keys_are_not_credentials() {
        for endpoint in ["token:path", "secret:backup", "password:archive"] {
            assert_eq!(redact(endpoint), endpoint);
            assert!(!contains_secret(endpoint));
        }
    }

    #[test]
    fn redacts_token_json() {
        let line = r#"config: {"token":{"access_token":"ya29.A0ARrd","expiry":"2025"}}"#;
        let out = redact(line);
        assert!(!out.contains("ya29.A0ARrd"));
    }

    #[test]
    fn redacts_bearer_header() {
        assert!(!redact("Authorization: Bearer eyJhbGciOi").contains("eyJhbGciOi"));
    }

    #[test]
    fn redacts_pem_key() {
        let key = "-----BEGIN OPENSSH PRIVATE KEY-----\nABCDEF\n-----END OPENSSH PRIVATE KEY-----";
        let out = redact(key);
        assert!(!out.contains("ABCDEF"));
        assert_eq!(out, REDACTED);
    }

    #[test]
    fn leaves_normal_lines_untouched() {
        let line = "Transferred: 1.2 GiB / 4.0 GiB, 30%, 12 MiB/s, ETA 3m";
        assert_eq!(redact(line), line);
    }

    #[test]
    fn redacts_multiple_secrets_in_one_line() {
        let line = "--password hunter2 --token abc.def --rc-pass=zzz keep-me";
        let out = redact(line);
        assert!(!out.contains("hunter2"));
        assert!(!out.contains("abc.def"));
        assert!(!out.contains("zzz"));
        assert!(out.contains("keep-me"), "non-secret text must survive");
    }

    #[test]
    fn redacts_each_known_flag_family() {
        for flag in [
            "--password X1Y2",
            "--pass X1Y2",
            "--rc-pass X1Y2",
            "--rc-user X1Y2",
            "--token X1Y2",
            "--client-secret X1Y2",
            "--sftp-pass X1Y2",
            "--sa-credentials X1Y2",
        ] {
            assert!(!redact(flag).contains("X1Y2"), "leaked: {flag}");
        }
    }

    #[test]
    fn redaction_is_case_insensitive() {
        assert!(!redact("--PASSWORD hunter2").contains("hunter2"));
        assert!(!redact("Authorization: BEARER eyToken").contains("eyToken"));
    }

    #[test]
    fn redacts_url_credentials_with_special_password() {
        // Password contains characters that must not confuse the regex.
        let line = "ftp://bob:p%40ss.w0rd!@host/path";
        let out = redact(line);
        assert!(!out.contains("p%40ss.w0rd!"));
        assert!(out.contains("bob:«redacted»@"));
        // The non-secret host/path is preserved.
        assert!(out.contains("@host/path"));
    }

    #[test]
    fn redacts_access_and_refresh_tokens() {
        assert!(!redact(r#""access_token":"ya29.abcDEF-123""#).contains("ya29.abcDEF-123"));
        assert!(!redact("refresh_token=1//0gWxyz_-.").contains("1//0gWxyz_-."));
    }

    #[test]
    fn empty_and_whitespace_are_unchanged() {
        assert_eq!(redact(""), "");
        assert_eq!(redact("   \t"), "   \t");
    }

    #[test]
    fn redacts_pem_embedded_in_surrounding_text() {
        let line =
            "before -----BEGIN RSA PRIVATE KEY-----\nSECRET\n-----END RSA PRIVATE KEY----- after";
        let out = redact(line);
        assert!(!out.contains("SECRET"));
        assert!(out.contains("before "));
        assert!(out.contains(" after"));
    }

    #[test]
    fn does_not_redact_plain_words_resembling_flags() {
        // A word like "password" in prose (no flag prefix / value) is untouched.
        let line = "the password policy requires 12 characters";
        assert_eq!(redact(line), line);
    }

    #[test]
    fn stream_redactor_suppresses_multiline_private_key() {
        let mut redactor = StreamRedactor::new();
        assert_eq!(
            redactor.redact_line("prefix -----BEGIN OPENSSH PRIVATE KEY-----"),
            Some(REDACTED.to_string())
        );
        assert_eq!(redactor.redact_line("SUPER-SECRET-BODY"), None);
        assert_eq!(
            redactor.redact_line("-----END OPENSSH PRIVATE KEY-----"),
            None
        );
        assert_eq!(
            redactor.redact_line("safe again"),
            Some("safe again".to_string())
        );
    }
}
