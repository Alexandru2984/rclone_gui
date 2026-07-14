//! Property-based tests for the security-critical pure functions: instead of a
//! few hand-picked examples, assert invariants over many generated inputs
//! (including control characters and newlines).

use proptest::prelude::*;

use cascade_core::dryrun::DryRunSummary;
use cascade_core::job::{AdvancedOptions, JobSpec, OpKind};
use cascade_core::schedule::{build_units, unit_id};
use cascade_core::security::path::{self, Overlap};
use cascade_core::security::{flags, sanitize};
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
        let units = build_units(&name, "/usr/bin/rsync", &args, "daily", None).unwrap();
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

    /// `unit_id` always yields a valid systemd id fragment: non-empty, lowercase,
    /// only [a-z0-9_-], no leading/trailing/double dash.
    #[test]
    fn unit_id_is_always_a_safe_slug(name in any::<String>()) {
        let id = unit_id(&name);
        prop_assert!(!id.is_empty());
        prop_assert!(id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_'));
        prop_assert!(!id.starts_with('-') && !id.ends_with('-'));
        prop_assert!(!id.contains("--"));
    }

    /// A dry-run summary parser never panics and only ever increments counts by
    /// the number of lines fed (each line contributes at most one change).
    #[test]
    fn dryrun_summary_is_bounded(lines in proptest::collection::vec(any::<String>(), 0..30)) {
        let mut rs = DryRunSummary::default();
        let mut rc = DryRunSummary::default();
        for l in &lines {
            rs.record_line(Tool::Rsync, l);
            rc.record_line(Tool::Rclone, l);
        }
        let total = |s: &DryRunSummary| s.added + s.updated + s.deleted;
        prop_assert!(total(&rs) <= lines.len() as u64);
        prop_assert!(total(&rc) <= lines.len() as u64);
    }

    /// Secret-bearing job specs are refused before argv/preview generation,
    /// while the defense-in-depth sanitizer still removes the same secret.
    #[test]
    fn preview_rejects_embedded_secrets(tail in "[!-~]{0,20}") {
        let secret = format!("SEKRETzzz{tail}");
        let mut spec = make_spec(Tool::Rsync, OpKind::Copy, "/src/", "/dst/");
        spec.options.extra_flags = vec![format!("--sftp-pass={secret}")];
        prop_assert!(spec.contains_secret());
        prop_assert!(spec.preview().is_err());
        prop_assert!(spec.preview_sanitized().is_err());
        let safe = sanitize::redact(&format!("rsync --sftp-pass={secret}"));
        prop_assert!(!safe.contains("SEKRETzzz"), "secret leaked into sanitized preview: {safe}");
    }

    /// Overlap classification is anti-symmetric for the nested cases: swapping
    /// source and destination flips DestInsideSource <-> SourceInsideDest, and
    /// Identical stays Identical.
    #[test]
    fn overlap_swap_is_consistent(
        root in "/[a-z]{1,8}/[a-z]{1,8}",
        child in "[a-z]{1,8}",
    ) {
        let nested = format!("{root}/{child}");
        prop_assert_eq!(path::check_overlap(&root, &nested), Overlap::DestInsideSource);
        prop_assert_eq!(path::check_overlap(&nested, &root), Overlap::SourceInsideDest);
        prop_assert_eq!(path::check_overlap(&root, &root), Overlap::Identical);
    }

    /// Any spec that builds an argv produces a preview that starts with the
    /// tool's binary — never a shell operator or empty string.
    #[test]
    fn every_buildable_spec_previews_with_its_binary(
        rclone in any::<bool>(),
        op_idx in 0u8..3,
        src in "[a-z/]{1,12}",
        dst in "[a-z/]{1,12}",
    ) {
        let tool = if rclone { Tool::Rclone } else { Tool::Rsync };
        let op = match op_idx { 0 => OpKind::Copy, 1 => OpKind::Sync, _ => OpKind::Move };
        let spec = make_spec(tool, op, &format!("/{src}"), &format!("/{dst}"));
        if let Ok(preview) = spec.preview() {
            prop_assert!(preview.starts_with(spec.binary()));
        }
    }
}

/// Build a simple spec for property tests.
fn make_spec(tool: Tool, op: OpKind, source: &str, dest: &str) -> JobSpec {
    JobSpec {
        name: "p".into(),
        tool,
        op,
        source: source.into(),
        destination: dest.into(),
        dry_run: false,
        delete: false,
        options: AdvancedOptions::default(),
    }
}
