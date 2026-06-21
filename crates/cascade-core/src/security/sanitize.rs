//! Log sanitization — redact secrets before any line is shown or written to disk.
//!
//! This runs on **every** stdout/stderr line. It is intentionally conservative:
//! false positives (over-redaction) are acceptable; leaking a token is not.

use std::sync::OnceLock;

use regex::Regex;

const REDACTED: &str = "«redacted»";

struct Patterns {
    rules: Vec<(Regex, &'static str)>,
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
                    r"(?i)(--(?:password|pass|rc-pass|rc-user|token|client-secret|sftp-pass|sa-credentials)[= ])\S+",
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
            // PEM private-key bodies
            (
                Regex::new(
                    r"(?s)-----BEGIN [^-]*PRIVATE KEY-----.*?-----END [^-]*PRIVATE KEY-----",
                )
                .unwrap(),
                REDACTED,
            ),
        ];
        Patterns { rules }
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
}
