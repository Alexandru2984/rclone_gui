//! Async process runner.
//!
//! Spawns an external tool with an explicit **argv** (no shell, `stdin = null`)
//! and streams its output as [`ProcessEvent`]s over an `async-channel`. Every
//! output line is passed through [`crate::security::sanitize`] *inside* the
//! runner, so a secret can never leave this module un-redacted.
//!
//! Children are driven on a shared, lazily-created multi-threaded Tokio runtime
//! (one per process, not one per command). This keeps the GUI free of any Tokio
//! dependency: it just consumes the receiver from its GLib main loop via
//! `glib::spawn_future_local`, because `async-channel` is executor-agnostic.

pub mod progress;

use std::process::Stdio;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use tokio::io::AsyncReadExt;
use tokio::process::Command;
use tokio::runtime::Runtime;

/// Hard cap on a single output line. Output without newlines (binary data, a
/// maliciously long filename) is truncated at this length instead of being
/// buffered without bound — protects against OOM (a denial of service).
const MAX_LINE_BYTES: usize = 64 * 1024;

/// Bound queued output per process. With the maximum line size this caps the
/// worst-case event payload backlog at roughly 8 MiB per process.
const EVENT_BUFFER_CAPACITY: usize = 128;

/// One-shot structured commands (lsjson, listremotes, RC calls) may return a
/// sizable response, but never get unlimited memory.
const MAX_CAPTURE_BYTES: usize = 16 * 1024 * 1024;
const CAPTURE_TIMEOUT: Duration = Duration::from_secs(120);

/// Shared multi-threaded Tokio runtime that drives every child process, created
/// lazily on first use. One runtime for the whole app instead of spinning up a
/// fresh thread + runtime per command.
fn runtime() -> &'static Runtime {
    static RT: OnceLock<Runtime> = OnceLock::new();
    RT.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("failed to build the Tokio runtime")
    })
}

use crate::job::Progress;
use crate::security::sanitize;

/// A line parser that turns a sanitized output line into a [`Progress`]
/// snapshot, or `None` if the line carries no progress info.
pub type LineParser = Arc<dyn Fn(&str) -> Option<Progress> + Send + Sync>;

/// Events emitted during a process run. Text payloads are already sanitized.
#[derive(Debug, Clone)]
pub enum ProcessEvent {
    Started {
        pid: Option<u32>,
    },
    Stdout(String),
    Stderr(String),
    /// A parsed progress update (bar/speed/ETA).
    Progress(Progress),
    Finished {
        success: bool,
        code: Option<i32>,
    },
    /// The process could not be started or was killed before completion.
    Error(String),
}

/// Handle to a running child: a stream of events plus a cancel trigger.
pub struct RunHandle {
    pub events: async_channel::Receiver<ProcessEvent>,
    cancel: async_channel::Sender<()>,
}

impl RunHandle {
    /// Request cancellation. The child is asked to stop gracefully (SIGTERM,
    /// then SIGKILL after a timeout). Safe to call more than once.
    pub fn cancel(&self) {
        let _ = self.cancel.try_send(());
    }
}

/// Spawn `binary` with `args`, with no progress parsing.
pub fn spawn(binary: impl Into<String>, args: Vec<String>) -> RunHandle {
    spawn_with_parser(binary, args, None)
}

/// Spawn `binary` with `args` and an optional progress [`LineParser`]. Returns
/// immediately with a [`RunHandle`]; the process is driven on a background thread.
pub fn spawn_with_parser(
    binary: impl Into<String>,
    args: Vec<String>,
    parser: Option<LineParser>,
) -> RunHandle {
    spawn_env(binary, args, Vec::new(), parser)
}

/// Like [`spawn_with_parser`], but also sets environment variables on the child.
///
/// Environment is preferred over argv for secrets (e.g. `RCLONE_RC_PASS`),
/// because `/proc/<pid>/environ` is readable only by the owner whereas
/// `/proc/<pid>/cmdline` is world-readable.
pub fn spawn_env(
    binary: impl Into<String>,
    args: Vec<String>,
    envs: Vec<(String, String)>,
    parser: Option<LineParser>,
) -> RunHandle {
    let binary = binary.into();
    let (ev_tx, ev_rx) = async_channel::bounded::<ProcessEvent>(EVENT_BUFFER_CAPACITY);
    let (cancel_tx, cancel_rx) = async_channel::bounded::<()>(1);

    runtime().spawn(drive(binary, args, envs, ev_tx, cancel_rx, parser));

    RunHandle {
        events: ev_rx,
        cancel: cancel_tx,
    }
}

/// Spawn a managed process whose output is intentionally discarded while a
/// [`RunHandle`] is retained for cancellation. A background drain prevents a
/// long-lived daemon from filling the bounded event queue and blocking its
/// stdout/stderr pipes.
pub(crate) fn spawn_env_quiet(
    binary: impl Into<String>,
    args: Vec<String>,
    envs: Vec<(String, String)>,
) -> RunHandle {
    let handle = spawn_env(binary, args, envs, None);
    let events = handle.events.clone();
    runtime().spawn(async move { while events.recv().await.is_ok() {} });
    handle
}

/// Run `binary args` to completion off the calling thread and return its
/// captured stdout on success, or an error string. For one-shot commands like
/// `rclone listremotes` / `lsjson` whose whole output is parsed at once.
pub fn capture(
    binary: impl Into<String>,
    args: Vec<String>,
) -> async_channel::Receiver<std::result::Result<String, String>> {
    capture_env(binary, args, Vec::new())
}

/// Like [`capture`], but also sets environment variables on the child (used to
/// pass RC credentials out of band rather than on the command line).
pub fn capture_env(
    binary: impl Into<String>,
    args: Vec<String>,
    envs: Vec<(String, String)>,
) -> async_channel::Receiver<std::result::Result<String, String>> {
    capture_env_with_limits(binary, args, envs, MAX_CAPTURE_BYTES, CAPTURE_TIMEOUT)
}

fn capture_env_with_limits(
    binary: impl Into<String>,
    args: Vec<String>,
    envs: Vec<(String, String)>,
    max_bytes: usize,
    timeout: Duration,
) -> async_channel::Receiver<std::result::Result<String, String>> {
    let binary = binary.into();
    let (tx, rx) = async_channel::bounded(1);
    runtime().spawn(async move {
        let msg = drive_capture(binary, args, envs, max_bytes, timeout).await;
        let _ = tx.send(msg).await;
    });
    rx
}

#[derive(Debug)]
struct CappedOutput {
    bytes: Vec<u8>,
    exceeded: bool,
}

async fn drive_capture(
    binary: String,
    args: Vec<String>,
    envs: Vec<(String, String)>,
    max_bytes: usize,
    timeout: Duration,
) -> std::result::Result<String, String> {
    let mut command = Command::new(&binary);
    command
        .args(&args)
        .envs(envs)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    harden_command(&mut command);
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(format!(
                "'{binary}' not found — is it installed and on PATH?"
            ));
        }
        Err(error) => return Err(format!("failed to run '{binary}': {error}")),
    };
    let stdout = child.stdout.take().expect("stdout piped");
    let stderr = child.stderr.take().expect("stderr piped");
    let completion = tokio::time::timeout(timeout, async {
        let (status, stdout, stderr) = tokio::join!(
            child.wait(),
            read_capped(stdout, max_bytes),
            read_capped(stderr, max_bytes)
        );
        (status, stdout, stderr)
    })
    .await;

    let (status, stdout, stderr) = match completion {
        Ok(result) => result,
        Err(_) => {
            force_kill(&mut child);
            let _ = child.wait().await;
            return Err(format!(
                "'{binary}' timed out after {} seconds",
                timeout.as_secs_f64()
            ));
        }
    };
    let status = status.map_err(|error| format!("failed waiting for '{binary}': {error}"))?;
    let stdout = stdout.map_err(|error| format!("failed reading '{binary}' stdout: {error}"))?;
    let stderr = stderr.map_err(|error| format!("failed reading '{binary}' stderr: {error}"))?;
    if stdout.exceeded || stderr.exceeded {
        return Err(format!(
            "'{binary}' output exceeded the {max_bytes}-byte safety limit"
        ));
    }

    if status.success() {
        // Defense in depth: captured structured output is redacted before it
        // can leave this module. Whole-buffer redaction also handles PEM blocks.
        Ok(sanitize::redact(&String::from_utf8_lossy(&stdout.bytes)))
    } else {
        let error = String::from_utf8_lossy(&stderr.bytes);
        Err(sanitize::redact(&format!(
            "{binary} failed: {}",
            error.trim()
        )))
    }
}

async fn read_capped<R: tokio::io::AsyncRead + Unpin>(
    mut reader: R,
    max_bytes: usize,
) -> std::io::Result<CappedOutput> {
    let mut bytes = Vec::with_capacity(max_bytes.min(8192));
    let mut exceeded = false;
    let mut chunk = [0_u8; 8192];
    loop {
        let count = reader.read(&mut chunk).await?;
        if count == 0 {
            break;
        }
        let remaining = max_bytes.saturating_sub(bytes.len());
        let keep = remaining.min(count);
        bytes.extend_from_slice(&chunk[..keep]);
        exceeded |= keep < count;
        // Continue draining after the cap so the child cannot deadlock on a
        // full pipe while it exits. Excess bytes are discarded.
    }
    Ok(CappedOutput { bytes, exceeded })
}

fn harden_command(command: &mut Command) {
    command.kill_on_drop(true);
    #[cfg(unix)]
    command.process_group(0);
}

async fn drive(
    binary: String,
    args: Vec<String>,
    envs: Vec<(String, String)>,
    ev: async_channel::Sender<ProcessEvent>,
    cancel_rx: async_channel::Receiver<()>,
    parser: Option<LineParser>,
) {
    let mut command = Command::new(&binary);
    command
        .args(&args)
        .envs(envs)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    harden_command(&mut command);
    let mut child = match command.spawn() {
        Ok(c) => c,
        Err(e) => {
            let msg = if e.kind() == std::io::ErrorKind::NotFound {
                format!("'{binary}' not found — is it installed and on PATH?")
            } else {
                format!("failed to start '{binary}': {e}")
            };
            let _ = ev.send(ProcessEvent::Error(msg)).await;
            let _ = ev
                .send(ProcessEvent::Finished {
                    success: false,
                    code: None,
                })
                .await;
            return;
        }
    };

    let _ = ev.send(ProcessEvent::Started { pid: child.id() }).await;

    let stdout = child.stdout.take().expect("stdout piped");
    let stderr = child.stderr.take().expect("stderr piped");

    let read_stdout = stream_lines(stdout, ev.clone(), parser.clone(), false);
    let read_stderr = stream_lines(stderr, ev.clone(), parser.clone(), true);

    // Only an EXPLICIT cancel() may terminate the child. If the RunHandle is
    // merely dropped, the channel closes with Err — that must detach the child,
    // not kill it (an OAuth `rclone config create` outlives its handle while
    // the user signs in in the browser), so the future parks forever.
    let explicit_cancel = async {
        if cancel_rx.recv().await.is_err() {
            std::future::pending::<()>().await;
        }
    };
    let wait_or_cancel = async {
        tokio::select! {
            status = child.wait() => status.map_err(|e| e.to_string()),
            _ = explicit_cancel => {
                let _ = ev.try_send(ProcessEvent::Error("cancelled by user".into()));
                graceful_terminate(&mut child).await
            }
        }
    };

    // Drive readers and the wait concurrently on this single thread.
    let (stdout_result, stderr_result, result) =
        tokio::join!(read_stdout, read_stderr, wait_or_cancel);

    let output_error = stdout_result.err().or_else(|| stderr_result.err());

    match (result, output_error) {
        (_, Some(error)) => {
            let _ = ev.send(ProcessEvent::Error(error)).await;
            let _ = ev
                .send(ProcessEvent::Finished {
                    success: false,
                    code: None,
                })
                .await;
        }
        (Ok(status), None) => {
            let _ = ev
                .send(ProcessEvent::Finished {
                    success: status.success(),
                    code: status.code(),
                })
                .await;
        }
        (Err(e), None) => {
            let _ = ev.send(ProcessEvent::Error(e)).await;
            let _ = ev
                .send(ProcessEvent::Finished {
                    success: false,
                    code: None,
                })
                .await;
        }
    }
}

/// Read `reader` in fixed chunks, splitting on `\n`, capping each line at
/// [`MAX_LINE_BYTES`] (excess is dropped and the line is marked truncated).
/// Memory stays bounded regardless of the child's output.
async fn stream_lines<R: AsyncReadExt + Unpin>(
    mut reader: R,
    ev: async_channel::Sender<ProcessEvent>,
    parser: Option<LineParser>,
    is_stderr: bool,
) -> std::result::Result<(), String> {
    let mut chunk = [0u8; 8192];
    let mut line: Vec<u8> = Vec::with_capacity(256);
    let mut truncated = false;
    let mut redactor = sanitize::StreamRedactor::new();

    loop {
        let n = match reader.read(&mut chunk).await {
            Ok(0) => break,
            Ok(n) => n,
            Err(error) => {
                let stream = if is_stderr { "stderr" } else { "stdout" };
                return Err(format!("failed reading child {stream}: {error}"));
            }
        };
        for &b in &chunk[..n] {
            if b == b'\n' {
                flush_line(
                    &ev,
                    &parser,
                    &mut redactor,
                    &mut line,
                    &mut truncated,
                    is_stderr,
                )
                .await;
            } else if line.len() < MAX_LINE_BYTES {
                line.push(b);
            } else {
                truncated = true; // drop bytes beyond the cap
            }
        }
    }
    if !line.is_empty() || truncated {
        flush_line(
            &ev,
            &parser,
            &mut redactor,
            &mut line,
            &mut truncated,
            is_stderr,
        )
        .await;
    }
    Ok(())
}

/// Sanitize, parse, and emit one accumulated line, then reset the buffer.
async fn flush_line(
    ev: &async_channel::Sender<ProcessEvent>,
    parser: &Option<LineParser>,
    redactor: &mut sanitize::StreamRedactor,
    line: &mut Vec<u8>,
    truncated: &mut bool,
    is_stderr: bool,
) {
    let mut text = String::from_utf8_lossy(line).into_owned();
    if *truncated {
        text.push_str(" …[truncated]");
    }
    line.clear();
    *truncated = false;
    if let Some(text) = redactor.redact_line(&text) {
        emit_line(ev, parser, text, is_stderr).await;
    }
}

/// Emit one output line: a parsed [`ProcessEvent::Progress`] when the parser
/// recognizes it, otherwise the raw (sanitized) line as stdout/stderr.
async fn emit_line(
    ev: &async_channel::Sender<ProcessEvent>,
    parser: &Option<LineParser>,
    line: String,
    is_stderr: bool,
) {
    if let Some(p) = parser {
        if let Some(progress) = p(&line) {
            let _ = ev.try_send(ProcessEvent::Progress(progress));
            return;
        }
    }
    let event = if is_stderr {
        ProcessEvent::Stderr(line)
    } else {
        ProcessEvent::Stdout(line)
    };
    let _ = ev.send(event).await;
}

/// Cancel a running child gracefully: send SIGTERM, wait up to 5 seconds for it
/// to clean up (partial temp files, FUSE locks), then SIGKILL if it ignores us.
/// Returns the child's real exit status — no fabricated value.
async fn graceful_terminate(
    child: &mut tokio::process::Child,
) -> std::result::Result<std::process::ExitStatus, String> {
    let pid = child.id();
    #[cfg(unix)]
    if let Some(pid) = pid {
        // SAFETY: the child was placed in its own process group at spawn.
        unsafe {
            libc::kill(-(pid as libc::pid_t), libc::SIGTERM);
        }
    }
    #[cfg(not(unix))]
    let _ = child.start_kill();

    match tokio::time::timeout(std::time::Duration::from_secs(5), child.wait()).await {
        Ok(status) => {
            #[cfg(unix)]
            kill_process_group(pid, libc::SIGKILL);
            status.map_err(|e| e.to_string())
        }
        Err(_) => {
            force_kill(child);
            child.wait().await.map_err(|e| e.to_string())
        }
    }
}

fn force_kill(child: &mut tokio::process::Child) {
    #[cfg(unix)]
    kill_process_group(child.id(), libc::SIGKILL);
    let _ = child.start_kill();
}

#[cfg(unix)]
fn kill_process_group(pid: Option<u32>, signal: libc::c_int) {
    if let Some(pid) = pid {
        // SAFETY: the child was placed in a dedicated process group whose id
        // equals its pid. ESRCH is harmless if the whole group already exited.
        unsafe {
            libc::kill(-(pid as libc::pid_t), signal);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(flavor = "multi_thread")]
    async fn event_queue_is_bounded() {
        let handle = spawn("true", vec![]);
        assert_eq!(handle.events.capacity(), Some(EVENT_BUFFER_CAPACITY));
        finish(&handle).await;
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn cancel_terminates_a_running_process() {
        // `sleep 30` exits promptly on SIGTERM; cancelling must finish it well
        // within the 5s SIGKILL fallback (proving graceful termination works).
        let h = spawn("sleep", vec!["30".into()]);
        // Wait until it has started.
        loop {
            match h.events.recv().await {
                Ok(ProcessEvent::Started { .. }) => break,
                Ok(_) => {}
                Err(_) => panic!("channel closed before start"),
            }
        }
        h.cancel();
        let finished = tokio::time::timeout(std::time::Duration::from_secs(4), async {
            while let Ok(ev) = h.events.recv().await {
                if let ProcessEvent::Finished { success, .. } = ev {
                    return success;
                }
            }
            true
        })
        .await;
        assert!(
            matches!(finished, Ok(false)),
            "cancel should finish the job (unsuccessfully) fast"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn long_line_is_capped_not_unbounded() {
        // 200 KB of NUL bytes with no newline — a naive reader would buffer it
        // all; ours must cap each emitted line at MAX_LINE_BYTES.
        let h = spawn(
            "head",
            vec!["-c".into(), "200000".into(), "/dev/zero".into()],
        );
        let mut longest = 0usize;
        while let Ok(ev) = h.events.recv().await {
            match ev {
                ProcessEvent::Stdout(l) => longest = longest.max(l.len()),
                ProcessEvent::Finished { .. } => break,
                _ => {}
            }
        }
        assert!(longest > 0, "expected some output");
        assert!(
            longest <= MAX_LINE_BYTES + 32,
            "line not capped: {longest} bytes"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn runs_echo_and_streams_stdout() {
        let h = spawn("echo", vec!["hello-cascade".into()]);
        let mut saw_line = false;
        let mut finished_ok = false;
        while let Ok(ev) = h.events.recv().await {
            match ev {
                ProcessEvent::Stdout(l) if l.contains("hello-cascade") => saw_line = true,
                ProcessEvent::Finished { success, .. } => {
                    finished_ok = success;
                    break;
                }
                _ => {}
            }
        }
        assert!(saw_line, "expected stdout line");
        assert!(finished_ok, "echo should exit 0");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn missing_binary_reports_error() {
        let h = spawn("definitely-not-a-real-binary-xyz", vec![]);
        let mut saw_error = false;
        while let Ok(ev) = h.events.recv().await {
            match ev {
                ProcessEvent::Error(_) => saw_error = true,
                ProcessEvent::Finished { success, .. } => {
                    assert!(!success);
                    break;
                }
                _ => {}
            }
        }
        assert!(saw_error);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn output_is_sanitized() {
        // `printf` a fake secret; the runner must redact it before emitting.
        let h = spawn("printf", vec!["--password hunter2\\n".into()]);
        while let Ok(ev) = h.events.recv().await {
            match ev {
                ProcessEvent::Stdout(l) => assert!(!l.contains("hunter2"), "secret leaked: {l}"),
                ProcessEvent::Finished { .. } => break,
                _ => {}
            }
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn multiline_private_key_is_suppressed_across_streamed_lines() {
        let h = spawn(
            "printf",
            vec![
                "before\n-----BEGIN OPENSSH PRIVATE KEY-----\nSUPER-SECRET-BODY\n-----END OPENSSH PRIVATE KEY-----\nafter\n"
                    .into(),
            ],
        );
        let mut emitted = String::new();
        while let Ok(ev) = h.events.recv().await {
            match ev {
                ProcessEvent::Stdout(line) | ProcessEvent::Stderr(line) => {
                    emitted.push_str(&line);
                }
                ProcessEvent::Finished { .. } => break,
                _ => {}
            }
        }
        assert!(!emitted.contains("SUPER-SECRET-BODY"));
        assert!(!emitted.contains("PRIVATE KEY"));
        assert!(emitted.contains("before"));
        assert!(emitted.contains("after"));
    }

    /// Drain a handle to its Finished event, returning (success, code).
    async fn finish(h: &RunHandle) -> (bool, Option<i32>) {
        while let Ok(ev) = h.events.recv().await {
            if let ProcessEvent::Finished { success, code } = ev {
                return (success, code);
            }
        }
        panic!("channel closed before Finished");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn exit_codes_are_propagated() {
        assert_eq!(finish(&spawn("true", vec![])).await, (true, Some(0)));
        assert_eq!(finish(&spawn("false", vec![])).await, (false, Some(1)));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn environment_is_passed_to_the_child() {
        let h = spawn_env(
            "printenv",
            vec!["CASCADE_TEST_VAR".into()],
            vec![("CASCADE_TEST_VAR".into(), "value-42".into())],
            None,
        );
        let mut saw = false;
        while let Ok(ev) = h.events.recv().await {
            match ev {
                ProcessEvent::Stdout(l) if l.contains("value-42") => saw = true,
                ProcessEvent::Finished { .. } => break,
                _ => {}
            }
        }
        assert!(saw, "child did not see the injected env var");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn capture_returns_stdout_on_success() {
        let rx = capture("echo", vec!["hello-capture".into()]);
        let out = rx.recv().await.unwrap();
        assert_eq!(out.unwrap().trim(), "hello-capture");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn capture_reports_failure_as_err() {
        let rx = capture("false", vec![]);
        assert!(rx.recv().await.unwrap().is_err());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn capture_success_output_is_sanitized() {
        // Even on success, a secret in captured stdout must be redacted.
        let rx = capture("printf", vec!["--password hunter2\\n".into()]);
        let out = rx.recv().await.unwrap().unwrap();
        assert!(!out.contains("hunter2"), "secret leaked via capture: {out}");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn capture_leaves_normal_json_intact() {
        // Redaction must not disturb ordinary structured output.
        let rx = capture("printf", vec![r#"{"version":"v1.66.0"}"#.into()]);
        let out = rx.recv().await.unwrap().unwrap();
        assert_eq!(out, r#"{"version":"v1.66.0"}"#);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn capture_reports_missing_binary() {
        let rx = capture("definitely-not-a-real-binary-xyz", vec![]);
        let err = rx.recv().await.unwrap().unwrap_err();
        assert!(err.contains("not found"), "unexpected error: {err}");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn capture_env_passes_environment() {
        let rx = capture_env(
            "printenv",
            vec!["CASCADE_CAP_VAR".into()],
            vec![("CASCADE_CAP_VAR".into(), "cap-value".into())],
        );
        assert_eq!(rx.recv().await.unwrap().unwrap().trim(), "cap-value");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn capture_refuses_output_over_its_byte_limit() {
        let rx = capture_env_with_limits(
            "head",
            vec!["-c".into(), "4096".into(), "/dev/zero".into()],
            Vec::new(),
            1024,
            Duration::from_secs(2),
        );
        let error = rx.recv().await.unwrap().unwrap_err();
        assert!(error.contains("1024-byte safety limit"), "{error}");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn capture_times_out_and_terminates_the_child() {
        let started = std::time::Instant::now();
        let rx = capture_env_with_limits(
            "sleep",
            vec!["30".into()],
            Vec::new(),
            1024,
            Duration::from_millis(50),
        );
        let error = rx.recv().await.unwrap().unwrap_err();
        assert!(error.contains("timed out"), "{error}");
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn captured_multiline_private_key_is_fully_redacted() {
        let rx = capture(
            "printf",
            vec![
                "-----BEGIN OPENSSH PRIVATE KEY-----\nCAPTURED-SECRET-BODY\n-----END OPENSSH PRIVATE KEY-----\n"
                    .into(),
            ],
        );
        let output = rx.recv().await.unwrap().unwrap();
        assert!(!output.contains("CAPTURED-SECRET-BODY"));
        assert!(!output.contains("PRIVATE KEY"));
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread")]
    async fn cancellation_terminates_descendant_processes() {
        let handle = spawn(
            "sh",
            vec![
                "-c".into(),
                "sleep 30 & child=$!; echo CHILD:$child; wait".into(),
            ],
        );
        let child_pid = loop {
            match handle.events.recv().await {
                Ok(ProcessEvent::Stdout(line)) if line.starts_with("CHILD:") => {
                    break line[6..].parse::<libc::pid_t>().unwrap();
                }
                Ok(_) => {}
                Err(_) => panic!("channel closed before descendant pid"),
            }
        };
        handle.cancel();
        let _ = finish(&handle).await;

        for _ in 0..40 {
            // SAFETY: signal 0 only probes whether the pid still exists.
            let exists = unsafe { libc::kill(child_pid, 0) } == 0;
            if !exists {
                return;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        panic!("descendant process {child_pid} survived cancellation");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn dropping_the_handle_detaches_instead_of_killing() {
        // Regression: dropping the RunHandle used to close the cancel channel,
        // which the runner treated as a cancel and SIGTERMed the child — this
        // broke OAuth `rclone config create` (killed before the browser opened).
        // `sleep` exits 0 only if it was NOT killed.
        let h = spawn("sleep", vec!["0.3".into()]);
        let events = h.events.clone();
        drop(h);
        let mut finished_ok = false;
        while let Ok(ev) = events.recv().await {
            match ev {
                ProcessEvent::Error(e) => panic!("spurious error after drop: {e}"),
                ProcessEvent::Finished { success, .. } => {
                    finished_ok = success;
                    break;
                }
                _ => {}
            }
        }
        assert!(finished_ok, "child was killed when the handle dropped");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn double_cancel_is_safe() {
        let h = spawn("sleep", vec!["30".into()]);
        loop {
            match h.events.recv().await {
                Ok(ProcessEvent::Started { .. }) => break,
                Ok(_) => {}
                Err(_) => panic!("channel closed before start"),
            }
        }
        // Calling cancel more than once must not panic or hang.
        h.cancel();
        h.cancel();
        let (success, _) = tokio::time::timeout(std::time::Duration::from_secs(4), finish(&h))
            .await
            .expect("cancel should finish promptly");
        assert!(!success);
    }
}
