//! Property-based tests for the security-critical pure functions: instead of a
//! few hand-picked examples, assert invariants over many generated inputs
//! (including control characters and newlines).

use proptest::prelude::*;

use cascade_core::job::{AdvancedOptions, JobSpec, OpKind};
use cascade_core::schedule::build_units;
use cascade_core::security::{flags, path, sanitize};
use cascade_core::Tool;

proptest! {
    /// A password passed via `--password` is never echoed back after that flag,
    /// for any non-whitespace secret.
    #[test]
    fn sanitizer_redacts_password_flag(secret in "[!-~]{1,48}") {
        let line = format!("rclone --password {secret} remote:path");
        let out = sanitize::redact(&line);
        prop_assert!(!out.contains(&format!("--password {secret}")), "secret leaked: {out}");
        prop_assert!(out.contains("«redacted»"));
    }

    /// Redaction is idempotent: running it twice changes nothing more.
    #[test]
    fn sanitizer_is_idempotent(s in any::<String>()) {
        let once = sanitize::redact(&s);
        let twice = sanitize::redact(&once);
        prop_assert_eq!(once, twice);
    }

    /// Flag parsing never panics, whatever the input.
    #[test]
    fn flag_parser_never_panics(s in any::<String>()) {
        let _ = flags::parse(&s);
    }

    /// A NUL byte in custom flags is always rejected.
    #[test]
    fn flag_parser_rejects_nul(prefix in "[ -~]*", suffix in "[ -~]*") {
        let s = format!("{prefix}\0{suffix}");
        prop_assert!(flags::parse(&s).is_err());
    }

    /// Path validation never panics and always refuses the filesystem root.
    #[test]
    fn path_validation_is_total(s in any::<String>()) {
        let _ = path::validate(&s);
        prop_assert!(path::validate("/").is_err());
    }

    /// A generated systemd unit always has exactly one `ExecStart` line and the
    /// fixed five-directive body — a crafted name/arg (even with newlines) can
    /// never split the line or inject another directive.
    #[test]
    fn scheduling_never_injects_directives(
        name in any::<String>(),
        args in proptest::collection::vec(any::<String>(), 0..6),
    ) {
        let units = build_units(&name, "/usr/bin/rsync", &args, "daily", None);
        let exec_lines = units.service.lines().filter(|l| l.starts_with("ExecStart=")).count();
        prop_assert_eq!(exec_lines, 1, "ExecStart was split or duplicated");
        // [Unit] / Description / blank / [Service] / Type=oneshot / ExecStart.
        prop_assert_eq!(units.service.lines().count(), 6);
        prop_assert!(units.service.contains("Type=oneshot"));
    }

    /// A spec with a credential in a flag is always reported as secret-bearing.
    #[test]
    fn specs_with_password_flags_are_flagged(secret in "[!-~]{1,32}") {
        let spec = JobSpec {
            name: "t".into(),
            tool: Tool::Rsync,
            op: OpKind::Copy,
            source: "/a/".into(),
            destination: "/b/".into(),
            dry_run: false,
            delete: false,
            options: AdvancedOptions {
                extra_flags: vec!["--sftp-pass".into(), secret],
                ..Default::default()
            },
        };
        prop_assert!(spec.contains_secret());
    }
}
