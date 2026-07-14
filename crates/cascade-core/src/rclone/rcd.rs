//! A local `rclone rcd` remote-control daemon.
//!
//! Threat model (see docs/THREAT_MODEL.md #4): the daemon is bound **only** to
//! `127.0.0.1` on a free port, protected by a random user/password generated
//! per session from the OS CSPRNG, and never advertised on the network. We talk
//! to it using `rclone rc` as the HTTP client, so no extra HTTP dependency is
//! pulled in, and the credentials are redacted from logs by the sanitizer.

use std::io::Read;

use crate::process::{spawn_env_quiet, RunHandle};

/// First rclone release containing the RC authorization fixes for
/// CVE-2026-41176 and CVE-2026-41179.
pub const MIN_SAFE_RCLONE_VERSION: (u64, u64, u64) = (1, 73, 5);

/// A running local RC daemon. Dropping or calling [`Rcd::stop`] kills it.
pub struct Rcd {
    addr: String,
    user: String,
    pass: String,
    handle: RunHandle,
}

impl Rcd {
    /// Refuse to expose an RC endpoint through an rclone version with known
    /// pre-authentication command-execution vulnerabilities.
    ///
    /// This check is deliberately fail-closed: an absent or unparseable version
    /// never starts a daemon. Callers may safely fall back to the ordinary CLI
    /// transfer path.
    pub fn security_check() -> std::io::Result<()> {
        let info = super::detect::detect().ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::NotFound, "rclone is not installed")
        })?;
        if version_is_safe(&info.version) {
            return Ok(());
        }
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            format!(
                "rclone RC requires version {}.{}.{} or newer; found '{}'",
                MIN_SAFE_RCLONE_VERSION.0,
                MIN_SAFE_RCLONE_VERSION.1,
                MIN_SAFE_RCLONE_VERSION.2,
                info.version
            ),
        ))
    }

    /// Start `rclone rcd` on a free loopback port with random credentials.
    ///
    /// Credentials are passed via the environment (`RCLONE_RC_USER`/`_PASS`),
    /// never on the command line, so they are not exposed in the world-readable
    /// `/proc/<pid>/cmdline`.
    pub fn start() -> std::io::Result<Self> {
        Self::security_check()?;
        let port = free_loopback_port()?;
        let addr = format!("127.0.0.1:{port}");
        let user = format!("cascade-{}", random_hex(4)?);
        let pass = random_hex(24)?;
        let args = vec!["rcd".to_string(), format!("--rc-addr={addr}")];
        let envs = vec![
            ("RCLONE_RC_USER".to_string(), user.clone()),
            ("RCLONE_RC_PASS".to_string(), pass.clone()),
        ];
        let handle = spawn_env_quiet("rclone", args, envs);
        Ok(Self {
            addr,
            user,
            pass,
            handle,
        })
    }

    /// The loopback address the daemon is bound to (e.g. `127.0.0.1:5572`).
    pub fn addr(&self) -> &str {
        &self.addr
    }

    /// argv for `rclone rc <command>` against this daemon (no credentials —
    /// those go through [`Self::rc_env`]).
    pub fn rc_args(&self, command: &str) -> Vec<String> {
        vec![
            "rc".to_string(),
            format!("--rc-addr={}", self.addr),
            command.to_string(),
        ]
    }

    /// argv for `rclone rc <method> --json <payload>` against this daemon, for
    /// RC calls that take a JSON body (e.g. `sync/copy`, `core/stats`). The
    /// payload carries only paths/flags — never a secret — so argv is fine.
    pub fn rc_args_json(&self, method: &str, json: &str) -> Vec<String> {
        vec![
            "rc".to_string(),
            format!("--rc-addr={}", self.addr),
            method.to_string(),
            "--json".to_string(),
            json.to_string(),
        ]
    }

    /// Environment carrying the RC credentials, for use with `capture_env`.
    pub fn rc_env(&self) -> Vec<(String, String)> {
        vec![
            ("RCLONE_RC_USER".to_string(), self.user.clone()),
            ("RCLONE_RC_PASS".to_string(), self.pass.clone()),
        ]
    }

    /// Stop the daemon and its process group via the process runner.
    pub fn stop(&self) {
        self.handle.cancel();
    }
}

impl Drop for Rcd {
    fn drop(&mut self) {
        self.handle.cancel();
    }
}

/// Whether an `rclone version` banner meets the minimum safe RC version.
/// Accepts normal and distribution banners such as `rclone v1.74.4` and
/// `rclone v1.74.4-DEV`.
pub fn version_is_safe(banner: &str) -> bool {
    let Some(v_pos) = banner.find('v') else {
        return false;
    };
    let version_tail = &banner[v_pos + 1..];
    let numeric: String = version_tail
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    let mut parts = numeric.split('.');
    let parsed = (
        parts.next().and_then(|p| p.parse::<u64>().ok()),
        parts.next().and_then(|p| p.parse::<u64>().ok()),
        parts.next().and_then(|p| p.parse::<u64>().ok()),
    );
    match parsed {
        (Some(major), Some(minor), Some(patch)) => {
            let version = (major, minor, patch);
            version > MIN_SAFE_RCLONE_VERSION
                || (version == MIN_SAFE_RCLONE_VERSION
                    && version_tail.strip_prefix(&numeric).is_some_and(|suffix| {
                        suffix.is_empty() || suffix.chars().next().is_some_and(char::is_whitespace)
                    }))
        }
        _ => false,
    }
}

/// Extract the `version` field from a `core/version` JSON response.
pub fn parse_version(stdout: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(stdout).ok()?;
    v.get("version")?.as_str().map(|s| s.to_string())
}

/// Ask the OS for a free TCP port on loopback by binding to port 0.
fn free_loopback_port() -> std::io::Result<u16> {
    let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
    Ok(listener.local_addr()?.port())
}

/// `n` random bytes from the OS CSPRNG, hex-encoded.
///
/// Fails loudly if the CSPRNG can't be read: a silent fallback would hand the
/// RC daemon an all-zero (guessable) password, which for a credential-guarding
/// tool is worse than refusing to start.
fn random_hex(n: usize) -> std::io::Result<String> {
    let mut buf = vec![0u8; n];
    std::fs::File::open("/dev/urandom")?.read_exact(&mut buf)?;
    Ok(buf.iter().map(|b| format!("{b:02x}")).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::process::spawn_env;

    #[test]
    fn rc_args_have_no_credentials_and_env_carries_them() {
        let rcd = Rcd {
            addr: "127.0.0.1:5572".into(),
            user: "cascade-abcd".into(),
            pass: "deadbeef".into(),
            handle: spawn_env("true", vec![], Vec::new(), None),
        };
        let args = rcd.rc_args("core/version");
        assert_eq!(args[0], "rc");
        assert!(args.iter().any(|a| a == "--rc-addr=127.0.0.1:5572"));
        assert_eq!(args.last().unwrap(), "core/version");
        // Credentials must NOT appear in argv (they would be world-readable).
        assert!(!args
            .iter()
            .any(|a| a.contains("deadbeef") || a.contains("cascade-abcd")));

        // They are carried via the environment instead.
        let env = rcd.rc_env();
        assert!(env.contains(&("RCLONE_RC_USER".into(), "cascade-abcd".into())));
        assert!(env.contains(&("RCLONE_RC_PASS".into(), "deadbeef".into())));
        rcd.stop();
    }

    #[test]
    fn parses_version_json() {
        let json = r#"{"version":"v1.60.1","decomposed":[1,60,1]}"#;
        assert_eq!(parse_version(json).as_deref(), Some("v1.60.1"));
        assert_eq!(parse_version("not json"), None);
    }

    #[test]
    fn free_port_is_nonzero() {
        assert!(free_loopback_port().unwrap() > 0);
    }

    #[test]
    fn random_hex_has_expected_length_and_is_not_all_zero() {
        let hex = random_hex(24).unwrap();
        assert_eq!(hex.len(), 48);
        // Astronomically unlikely to be all zeros unless the CSPRNG read failed
        // silently — which this function no longer allows.
        assert_ne!(hex, "0".repeat(48));
    }

    #[test]
    fn rc_version_gate_is_fail_closed() {
        for unsafe_version in [
            "rclone v1.60.1-DEV",
            "rclone v1.69.0",
            "rclone v1.73.4",
            "rclone v1.73.5-beta.1",
            "unknown",
            "",
        ] {
            assert!(!version_is_safe(unsafe_version), "{unsafe_version}");
        }
        for safe_version in ["rclone v1.73.5", "rclone v1.74.4", "rclone v2.0.0"] {
            assert!(version_is_safe(safe_version), "{safe_version}");
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn dropping_rcd_cancels_its_process() {
        use crate::process::ProcessEvent;

        let handle = spawn_env("sleep", vec!["30".into()], Vec::new(), None);
        let events = handle.events.clone();
        loop {
            match events.recv().await {
                Ok(ProcessEvent::Started { .. }) => break,
                Ok(_) => {}
                Err(_) => panic!("process channel closed before start"),
            }
        }
        let rcd = Rcd {
            addr: "127.0.0.1:1".into(),
            user: "u".into(),
            pass: "p".into(),
            handle,
        };
        drop(rcd);

        let finished = tokio::time::timeout(std::time::Duration::from_secs(2), async {
            while let Ok(event) = events.recv().await {
                if let ProcessEvent::Finished { success, .. } = event {
                    return Some(success);
                }
            }
            None
        })
        .await;
        assert!(matches!(finished, Ok(Some(false))));
    }
}
