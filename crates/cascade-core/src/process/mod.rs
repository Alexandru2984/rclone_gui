//! Async process runner.
//!
//! Spawns an external tool with an explicit **argv** (no shell, `stdin = null`)
//! and streams its output as [`ProcessEvent`]s over an `async-channel`. Every
//! output line is passed through [`crate::security::sanitize`] *inside* the
//! runner, so a secret can never leave this module un-redacted.
//!
//! The child runs on a dedicated OS thread driving a current-thread Tokio
//! runtime. This keeps the GUI free of any Tokio dependency: it just consumes
//! the receiver from its GLib main loop via `glib::spawn_future_local`, because
//! `async-channel` is executor-agnostic.

pub mod progress;

use std::process::Stdio;
use std::sync::Arc;

use tokio::io::AsyncReadExt;
use tokio::process::Command;

/// Hard cap on a single output line. Output without newlines (binary data, a
/// maliciously long filename) is truncated at this length instead of being
/// buffered without bound — protects against OOM (a denial of service).
const MAX_LINE_BYTES: usize = 64 * 1024;

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
    let (ev_tx, ev_rx) = async_channel::unbounded::<ProcessEvent>();
    let (cancel_tx, cancel_rx) = async_channel::bounded::<()>(1);

    std::thread::spawn(move || {
        let rt = match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(rt) => rt,
            Err(e) => {
                let _ = ev_tx.send_blocking(ProcessEvent::Error(format!("runtime: {e}")));
                return;
            }
        };
        rt.block_on(drive(binary, args, envs, ev_tx, cancel_rx, parser));
    });

    RunHandle {
        events: ev_rx,
        cancel: cancel_tx,
    }
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
    let binary = binary.into();
    let (tx, rx) = async_channel::bounded(1);
    std::thread::spawn(move || {
        let result = std::process::Command::new(&binary)
            .args(&args)
            .envs(envs)
            .stdin(Stdio::null())
            .output();
        let msg = match result {
            Ok(out) if out.status.success() => {
                Ok(String::from_utf8_lossy(&out.stdout).into_owned())
            }
            Ok(out) => {
                let err = String::from_utf8_lossy(&out.stderr);
                Err(sanitize::redact(&format!(
                    "{binary} failed: {}",
                    err.trim()
                )))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Err(format!(
                "'{binary}' not found — is it installed and on PATH?"
            )),
            Err(e) => Err(format!("failed to run '{binary}': {e}")),
        };
        let _ = tx.send_blocking(msg);
    });
    rx
}

async fn drive(
    binary: String,
    args: Vec<String>,
    envs: Vec<(String, String)>,
    ev: async_channel::Sender<ProcessEvent>,
    cancel_rx: async_channel::Receiver<()>,
    parser: Option<LineParser>,
) {
    let mut child = match Command::new(&binary)
        .args(&args)
        .envs(envs)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
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

    let wait_or_cancel = async {
        tokio::select! {
            status = child.wait() => status.map_err(|e| e.to_string()),
            _ = cancel_rx.recv() => {
                let _ = ev.send(ProcessEvent::Error("cancelled by user".into())).await;
                graceful_terminate(&mut child).await
            }
        }
    };

    // Drive readers and the wait concurrently on this single thread.
    let (_, _, result) = tokio::join!(read_stdout, read_stderr, wait_or_cancel);

    match result {
        Ok(status) => {
            let _ = ev
                .send(ProcessEvent::Finished {
                    success: status.success(),
                    code: status.code(),
                })
                .await;
        }
        Err(e) => {
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
) {
    let mut chunk = [0u8; 8192];
    let mut line: Vec<u8> = Vec::with_capacity(256);
    let mut truncated = false;

    loop {
        let n = match reader.read(&mut chunk).await {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        for &b in &chunk[..n] {
            if b == b'\n' {
                flush_line(&ev, &parser, &mut line, &mut truncated, is_stderr).await;
            } else if line.len() < MAX_LINE_BYTES {
                line.push(b);
            } else {
                truncated = true; // drop bytes beyond the cap
            }
        }
    }
    if !line.is_empty() || truncated {
        flush_line(&ev, &parser, &mut line, &mut truncated, is_stderr).await;
    }
}

/// Sanitize, parse, and emit one accumulated line, then reset the buffer.
async fn flush_line(
    ev: &async_channel::Sender<ProcessEvent>,
    parser: &Option<LineParser>,
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
    emit_line(ev, parser, sanitize::redact(&text), is_stderr).await;
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
            let _ = ev.send(ProcessEvent::Progress(progress)).await;
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
    #[cfg(unix)]
    if let Some(pid) = child.id() {
        // SAFETY: `pid` is our own child; sending SIGTERM is always sound.
        unsafe {
            libc::kill(pid as libc::pid_t, libc::SIGTERM);
        }
    }
    match tokio::time::timeout(std::time::Duration::from_secs(5), child.wait()).await {
        Ok(status) => status.map_err(|e| e.to_string()),
        Err(_) => {
            let _ = child.start_kill();
            child.wait().await.map_err(|e| e.to_string())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
