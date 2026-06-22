//! A local `rclone rcd` remote-control daemon.
//!
//! Threat model (see docs/THREAT_MODEL.md #4): the daemon is bound **only** to
//! `127.0.0.1` on a free port, protected by a random user/password generated
//! per session from the OS CSPRNG, and never advertised on the network. We talk
//! to it using `rclone rc` as the HTTP client, so no extra HTTP dependency is
//! pulled in, and the credentials are redacted from logs by the sanitizer.

use std::io::Read;

use crate::process::{spawn, RunHandle};

/// A running local RC daemon. Dropping or calling [`Rcd::stop`] kills it.
pub struct Rcd {
    addr: String,
    user: String,
    pass: String,
    handle: RunHandle,
}

impl Rcd {
    /// Start `rclone rcd` on a free loopback port with random credentials.
    pub fn start() -> std::io::Result<Self> {
        let port = free_loopback_port()?;
        let addr = format!("127.0.0.1:{port}");
        let user = format!("cascade-{}", random_hex(4));
        let pass = random_hex(24);
        let args = vec![
            "rcd".to_string(),
            format!("--rc-addr={addr}"),
            format!("--rc-user={user}"),
            format!("--rc-pass={pass}"),
        ];
        let handle = spawn("rclone", args);
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

    /// argv for `rclone rc <command>` against this daemon. The command is a
    /// single argv item such as `core/version` or `core/stats`.
    pub fn rc_args(&self, command: &str) -> Vec<String> {
        vec![
            "rc".to_string(),
            format!("--rc-addr={}", self.addr),
            format!("--rc-user={}", self.user),
            format!("--rc-pass={}", self.pass),
            command.to_string(),
        ]
    }

    /// Stop the daemon (SIGKILL via the process runner).
    pub fn stop(&self) {
        self.handle.cancel();
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
fn random_hex(n: usize) -> String {
    let mut buf = vec![0u8; n];
    if let Ok(mut f) = std::fs::File::open("/dev/urandom") {
        let _ = f.read_exact(&mut buf);
    }
    buf.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rc_args_target_loopback_and_command_last() {
        // Construct without starting a daemon by faking the struct via start()
        // is overkill; test rc_args formatting through a hand-made instance.
        let rcd = Rcd {
            addr: "127.0.0.1:5572".into(),
            user: "cascade-abcd".into(),
            pass: "deadbeef".into(),
            handle: spawn("true", vec![]),
        };
        let args = rcd.rc_args("core/version");
        assert_eq!(args[0], "rc");
        assert!(args.iter().any(|a| a == "--rc-addr=127.0.0.1:5572"));
        assert!(args.iter().any(|a| a.starts_with("--rc-user=")));
        assert!(args.iter().any(|a| a.starts_with("--rc-pass=")));
        assert_eq!(args.last().unwrap(), "core/version");
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
    fn random_hex_has_expected_length() {
        assert_eq!(random_hex(8).len(), 16);
    }
}
