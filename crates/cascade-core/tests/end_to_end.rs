//! End-to-end integration tests: drive a real rsync through the whole core
//! pipeline (JobSpec -> argv -> process runner -> events) on temp files.
//!
//! These exercise everything a unit test can't: the actual child process, the
//! line streaming, and the on-disk result. They require `rsync` on PATH (always
//! present on Linux dev machines and CI runners).

use std::sync::Arc;

use cascade_core::job::{AdvancedOptions, JobSpec, OpKind};
use cascade_core::process::{progress, spawn_with_parser, ProcessEvent};
use cascade_core::Tool;

fn rsync_available() -> bool {
    cascade_core::rsync::detect().is_some()
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
