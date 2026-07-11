//! End-to-end integration tests: drive a real rsync through the whole core
//! pipeline (JobSpec -> argv -> process runner -> events) on temp files.
//!
//! These exercise everything a unit test can't: the actual child process, the
//! line streaming, and the on-disk result. They require `rsync` on PATH (always
//! present on Linux dev machines and CI runners).

use std::path::Path;
use std::sync::Arc;

use cascade_core::dryrun::DryRunSummary;
use cascade_core::job::{AdvancedOptions, JobSpec, OpKind};
use cascade_core::process::{progress, spawn_with_parser, LineParser, ProcessEvent};
use cascade_core::Tool;

fn rsync_available() -> bool {
    cascade_core::rsync::detect().is_some()
}

fn rclone_available() -> bool {
    cascade_core::rclone::detect().is_some()
}

/// Build an rclone spec over two local paths (no remote config needed — rclone's
/// `local` backend handles bare filesystem paths).
fn rclone_spec(op: OpKind, src: &Path, dst: &Path) -> JobSpec {
    JobSpec {
        name: "rc".into(),
        tool: Tool::Rclone,
        op,
        source: src.display().to_string(),
        destination: dst.display().to_string(),
        dry_run: false,
        delete: false,
        options: AdvancedOptions::default(),
    }
}

/// Run any spec (rsync or rclone) to completion, picking the matching progress
/// parser, and fold every output line into a `DryRunSummary`.
fn run_any(spec: &JobSpec) -> (bool, DryRunSummary) {
    let argv = spec.build_argv().expect("valid argv");
    let parser: LineParser = match spec.tool {
        Tool::Rsync => Arc::new(progress::parse_rsync),
        Tool::Rclone => Arc::new(progress::parse_rclone),
    };
    let handle = spawn_with_parser(spec.binary(), argv, Some(parser));
    let mut summary = DryRunSummary::default();
    let mut success = false;
    while let Ok(ev) = handle.events.recv_blocking() {
        match ev {
            ProcessEvent::Stdout(l) | ProcessEvent::Stderr(l) => {
                summary.record_line(spec.tool, &l);
            }
            ProcessEvent::Finished { success: ok, .. } => {
                success = ok;
                break;
            }
            _ => {}
        }
    }
    (success, summary)
}

fn copy_spec(src: &std::path::Path, dst: &std::path::Path, dry_run: bool) -> JobSpec {
    JobSpec {
        name: "it".into(),
        tool: Tool::Rsync,
        op: OpKind::Copy,
        source: format!("{}/", src.display()),
        destination: format!("{}/", dst.display()),
        dry_run,
        delete: false,
        options: AdvancedOptions::default(),
    }
}

fn sync_spec(src: &std::path::Path, dst: &std::path::Path, dry_run: bool) -> JobSpec {
    JobSpec {
        name: "it-sync".into(),
        tool: Tool::Rsync,
        op: OpKind::Sync, // mirror: deletes dest-only files
        source: format!("{}/", src.display()),
        destination: format!("{}/", dst.display()),
        dry_run,
        delete: false,
        options: AdvancedOptions::default(),
    }
}

/// Run a spec, feeding every output line through a `DryRunSummary`.
fn run_collecting_summary(spec: &JobSpec) -> (bool, DryRunSummary) {
    let argv = spec.build_argv().expect("valid argv");
    let parser = Arc::new(progress::parse_rsync);
    let handle = spawn_with_parser("rsync", argv, Some(parser));
    let mut summary = DryRunSummary::default();
    let mut success = false;
    while let Ok(ev) = handle.events.recv_blocking() {
        match ev {
            ProcessEvent::Stdout(l) | ProcessEvent::Stderr(l) => {
                summary.record_line(Tool::Rsync, &l);
            }
            ProcessEvent::Finished { success: ok, .. } => {
                success = ok;
                break;
            }
            _ => {}
        }
    }
    (success, summary)
}

/// Run a spec to completion, returning (success, saw_any_output).
fn run_to_completion(spec: &JobSpec) -> (bool, bool) {
    let argv = spec.build_argv().expect("valid argv");
    let parser = Arc::new(progress::parse_rsync);
    let handle = spawn_with_parser("rsync", argv, Some(parser));

    let mut success = false;
    let mut saw_output = false;
    while let Ok(ev) = handle.events.recv_blocking() {
        match ev {
            ProcessEvent::Stdout(_) | ProcessEvent::Stderr(_) | ProcessEvent::Progress(_) => {
                saw_output = true;
            }
            ProcessEvent::Finished { success: ok, .. } => {
                success = ok;
                break;
            }
            _ => {}
        }
    }
    (success, saw_output)
}

#[test]
fn rsync_copy_actually_transfers_files() {
    if !rsync_available() {
        eprintln!("skipping: rsync not installed");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("src");
    let dst = dir.path().join("dst");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::create_dir_all(&dst).unwrap();
    std::fs::write(src.join("a.txt"), b"hello world").unwrap();
    std::fs::write(src.join("big.bin"), vec![7u8; 50_000]).unwrap();

    let (success, _) = run_to_completion(&copy_spec(&src, &dst, false));
    assert!(success, "rsync copy should exit 0");

    // The whole point: the files really landed at the destination.
    assert_eq!(std::fs::read(dst.join("a.txt")).unwrap(), b"hello world");
    assert_eq!(
        std::fs::metadata(dst.join("big.bin")).unwrap().len(),
        50_000
    );
}

#[test]
fn dry_run_does_not_write_anything() {
    if !rsync_available() {
        eprintln!("skipping: rsync not installed");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("src");
    let dst = dir.path().join("dst");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::create_dir_all(&dst).unwrap();
    std::fs::write(src.join("a.txt"), b"data").unwrap();

    let (success, _) = run_to_completion(&copy_spec(&src, &dst, true));
    assert!(success, "rsync --dry-run should exit 0");

    // Dry-run must not create files at the destination.
    assert!(
        !dst.join("a.txt").exists(),
        "dry-run wrote to the destination"
    );
}

#[test]
fn dry_run_summary_reflects_real_changes() {
    if !rsync_available() {
        eprintln!("skipping: rsync not installed");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("src");
    let dst = dir.path().join("dst");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::create_dir_all(&dst).unwrap();

    // A brand-new file, a changed file, and a destination-only file.
    std::fs::write(src.join("new.txt"), b"fresh").unwrap();
    std::fs::write(src.join("changed.txt"), b"a much longer new version").unwrap();
    std::fs::write(dst.join("changed.txt"), b"old").unwrap();
    std::fs::write(dst.join("extra.txt"), b"remove me").unwrap();

    // A mirror dry-run should report exactly one of each kind of change.
    let (success, summary) = run_collecting_summary(&sync_spec(&src, &dst, true));
    assert!(success, "rsync sync --dry-run should exit 0");
    assert!(summary.added >= 1, "expected a new file: {summary:?}");
    assert!(
        summary.updated >= 1,
        "expected an updated file: {summary:?}"
    );
    assert!(summary.deleted >= 1, "expected a deletion: {summary:?}");

    // And it must not have touched the real destination.
    assert!(
        dst.join("extra.txt").exists(),
        "dry-run deleted a real file"
    );
    assert!(!dst.join("new.txt").exists(), "dry-run created a real file");
}

#[test]
fn sync_delete_removes_destination_only_files() {
    if !rsync_available() {
        eprintln!("skipping: rsync not installed");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("src");
    let dst = dir.path().join("dst");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::create_dir_all(&dst).unwrap();
    std::fs::write(src.join("keep.txt"), b"keep").unwrap();
    std::fs::write(dst.join("stale.txt"), b"should be deleted").unwrap();

    let (success, _) = run_to_completion(&sync_spec(&src, &dst, false));
    assert!(success, "rsync sync should exit 0");
    // The mirror made the destination identical to the source.
    assert!(dst.join("keep.txt").exists(), "source file must arrive");
    assert!(
        !dst.join("stale.txt").exists(),
        "a destination-only file must be deleted by a mirror"
    );
}

// --- rclone integration (guarded; skips where rclone is not installed, e.g. CI) ---

#[test]
fn rclone_copy_transfers_files() {
    if !rclone_available() {
        eprintln!("skipping: rclone not installed");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("src");
    let dst = dir.path().join("dst");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::create_dir_all(&dst).unwrap();
    std::fs::write(src.join("a.txt"), b"hello rclone").unwrap();

    let (success, _) = run_any(&rclone_spec(OpKind::Copy, &src, &dst));
    assert!(success, "rclone copy should exit 0");
    assert_eq!(std::fs::read(dst.join("a.txt")).unwrap(), b"hello rclone");
}

#[test]
fn rclone_sync_dry_run_summary_counts_changes() {
    if !rclone_available() {
        eprintln!("skipping: rclone not installed");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("src");
    let dst = dir.path().join("dst");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::create_dir_all(&dst).unwrap();
    std::fs::write(src.join("new.txt"), b"fresh").unwrap();
    std::fs::write(dst.join("extra.txt"), b"remove me").unwrap();

    let mut spec = rclone_spec(OpKind::Sync, &src, &dst);
    spec.dry_run = true;
    let (success, summary) = run_any(&spec);
    assert!(success, "rclone sync --dry-run should exit 0");
    // rclone labels a would-be transfer "copy" and a removal "delete".
    assert!(summary.added >= 1, "expected a copied file: {summary:?}");
    assert!(summary.deleted >= 1, "expected a deletion: {summary:?}");
    // The real destination is untouched by a dry-run.
    assert!(dst.join("extra.txt").exists());
    assert!(!dst.join("new.txt").exists());
}

#[test]
fn rclone_max_delete_aborts_a_runaway_mirror() {
    if !rclone_available() {
        eprintln!("skipping: rclone not installed");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("src");
    let dst = dir.path().join("dst");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::create_dir_all(&dst).unwrap();
    std::fs::write(src.join("keep.txt"), b"keep").unwrap();
    // Four destination-only files, but a max-delete of 1.
    for f in ["a", "b", "c", "d"] {
        std::fs::write(dst.join(format!("extra_{f}.txt")), b"x").unwrap();
    }

    let mut spec = rclone_spec(OpKind::Sync, &src, &dst);
    spec.options.max_delete = Some(1);
    let (success, _) = run_any(&spec);

    // The guard must abort the run instead of wiping the destination.
    assert!(
        !success,
        "sync should fail once the max-delete threshold is hit"
    );
    let remaining = std::fs::read_dir(&dst)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().starts_with("extra_"))
        .count();
    assert!(
        remaining >= 1,
        "the max-delete guard should have stopped a full wipe"
    );
}

#[test]
fn rclone_backup_dir_moves_instead_of_deleting() {
    if !rclone_available() {
        eprintln!("skipping: rclone not installed");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("src");
    let dst = dir.path().join("dst");
    let bak = dir.path().join("bak");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::create_dir_all(&dst).unwrap();
    std::fs::write(src.join("keep.txt"), b"keep").unwrap();
    std::fs::write(dst.join("stale.txt"), b"old").unwrap();

    let mut spec = rclone_spec(OpKind::Sync, &src, &dst);
    spec.options.backup_dir = Some(bak.display().to_string());
    let (success, _) = run_any(&spec);
    assert!(success, "rclone sync with --backup-dir should exit 0");

    // The stale file is moved aside, not destroyed — a reversible sync.
    assert!(
        !dst.join("stale.txt").exists(),
        "stale file should leave dst"
    );
    assert!(
        bak.join("stale.txt").exists(),
        "stale file should be preserved in the backup dir"
    );
    assert!(dst.join("keep.txt").exists());
}

#[test]
fn rclone_bisync_resync_merges_both_sides() {
    if !rclone_available() {
        eprintln!("skipping: rclone not installed");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let a = dir.path().join("a");
    let b = dir.path().join("b");
    std::fs::create_dir_all(&a).unwrap();
    std::fs::create_dir_all(&b).unwrap();
    std::fs::write(a.join("one.txt"), b"1").unwrap();
    std::fs::write(b.join("two.txt"), b"2").unwrap();

    let mut spec = rclone_spec(OpKind::Bisync, &a, &b);
    spec.options.resync = true; // first run establishes the baseline
    let (success, _) = run_any(&spec);
    assert!(success, "bisync --resync should exit 0");

    // After a resync both sides hold the union of the files.
    for side in [&a, &b] {
        assert!(side.join("one.txt").exists(), "one.txt missing on a side");
        assert!(side.join("two.txt").exists(), "two.txt missing on a side");
    }
}

#[test]
fn rcd_daemon_starts_answers_and_stops() {
    use cascade_core::process::capture_env;
    use cascade_core::rclone::rcd::{parse_version, Rcd};
    use std::time::Duration;

    if !rclone_available() {
        eprintln!("skipping: rclone not installed");
        return;
    }

    let rcd = Rcd::start().expect("rcd should start");
    assert!(
        rcd.addr().starts_with("127.0.0.1:"),
        "must bind loopback only"
    );

    // Give the daemon a moment to bind, then query it over the RC API. Retry a
    // few times to avoid a race with process startup.
    let mut version = None;
    for _ in 0..10 {
        std::thread::sleep(Duration::from_millis(300));
        let rx = capture_env("rclone", rcd.rc_args("core/version"), rcd.rc_env());
        if let Ok(Ok(out)) = rx.recv_blocking() {
            if let Some(v) = parse_version(&out) {
                version = Some(v);
                break;
            }
        }
    }
    rcd.stop();

    assert!(
        version.is_some(),
        "the local RC daemon should answer core/version"
    );
}

#[test]
fn rc_driven_transfer_runs_and_reports_stats() {
    use cascade_core::process::capture_env;
    use cascade_core::rclone::command::RcloneOptions;
    use cascade_core::rclone::rc;
    use cascade_core::rclone::rcd::Rcd;
    use std::time::Duration;

    if !rclone_available() {
        eprintln!("skipping: rclone not installed");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("src");
    let dst = dir.path().join("dst");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::create_dir_all(&dst).unwrap();
    std::fs::write(src.join("a.bin"), vec![1u8; 200_000]).unwrap();
    std::fs::write(src.join("b.bin"), vec![2u8; 200_000]).unwrap();

    let rcd = Rcd::start().expect("rcd starts");
    std::thread::sleep(Duration::from_millis(800));

    // Submit the copy as an async RC job scoped to a unique group.
    let group = "job/e2e";
    let payload = rc::sync_payload(
        &src.display().to_string(),
        &dst.display().to_string(),
        group,
        false,
        &RcloneOptions::default(),
    );
    let submit = capture_env(
        "rclone",
        rcd.rc_args_json("sync/copy", &payload),
        rcd.rc_env(),
    );
    let jobid = submit
        .recv_blocking()
        .unwrap()
        .ok()
        .and_then(|out| rc::parse_jobid(&out))
        .expect("async submit returns a jobid");

    // Poll job/status until it finishes.
    let mut done = None;
    for _ in 0..40 {
        std::thread::sleep(Duration::from_millis(150));
        let rx = capture_env(
            "rclone",
            rcd.rc_args_json("job/status", &format!("{{\"jobid\":{jobid}}}")),
            rcd.rc_env(),
        );
        if let Ok(Ok(out)) = rx.recv_blocking() {
            if let Some(st) = rc::parse_job_status(&out) {
                if st.finished {
                    done = Some(st);
                    break;
                }
            }
        }
    }
    let status = done.expect("RC job should finish");
    assert!(status.success, "RC transfer failed: {}", status.error);

    // core/stats for the group reflects the completed transfer.
    let rx = capture_env(
        "rclone",
        rcd.rc_args_json("core/stats", &format!("{{\"group\":\"{group}\"}}")),
        rcd.rc_env(),
    );
    let stats = rx
        .recv_blocking()
        .unwrap()
        .ok()
        .and_then(|o| rc::parse_core_stats(&o))
        .expect("core/stats parses");
    assert_eq!(stats.transfers_total, 2, "two files were scheduled");
    assert!(stats.bytes >= 400_000, "all bytes accounted for: {stats:?}");

    rcd.stop();

    // The files really arrived.
    assert!(dst.join("a.bin").exists() && dst.join("b.bin").exists());
}

#[test]
fn missing_binary_reports_failure_not_hang() {
    let handle = spawn_with_parser("definitely-not-a-tool-xyz", vec!["x".into()], None);
    let mut saw_error = false;
    let mut finished = false;
    while let Ok(ev) = handle.events.recv_blocking() {
        match ev {
            ProcessEvent::Error(_) => saw_error = true,
            ProcessEvent::Finished { success, .. } => {
                assert!(!success);
                finished = true;
                break;
            }
            _ => {}
        }
    }
    assert!(saw_error && finished);
}
