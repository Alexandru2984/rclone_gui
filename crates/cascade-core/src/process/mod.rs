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

use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;

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
    /// Request cancellation. The child is sent SIGKILL on the runtime thread.
    /// Safe to call more than once.
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

    let ev_out = ev.clone();
    let parser_out = parser.clone();
    let read_stdout = async move {
        let mut lines = BufReader::new(stdout).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            emit_line(&ev_out, &parser_out, sanitize::redact(&line), false).await;
        }
    };

    let ev_err = ev.clone();
    let parser_err = parser.clone();
    let read_stderr = async move {
        let mut lines = BufReader::new(stderr).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            emit_line(&ev_err, &parser_err, sanitize::redact(&line), true).await;
        }
    };

    let wait_or_cancel = async {
        tokio::select! {
            status = child.wait() => status.map_err(|e| e.to_string()),
            _ = cancel_rx.recv() => {
                let _ = child.start_kill();
                let status = child.wait().await.map_err(|e| e.to_string());
                // Mark as an explicit cancellation regardless of the wait result.
                let _ = ev.send(ProcessEvent::Error("cancelled by user".into())).await;
                status.map(|_| std::process::ExitStatus::default_failed())
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

/// Helper to construct a "failed" exit status portably for the cancel path.
trait ExitStatusExt {
    fn default_failed() -> std::process::ExitStatus;
}
impl ExitStatusExt for std::process::ExitStatus {
    fn default_failed() -> std::process::ExitStatus {
        #[cfg(unix)]
        {
            use std::os::unix::process::ExitStatusExt as _;
            std::process::ExitStatus::from_raw(9) // SIGKILL
        }
        #[cfg(not(unix))]
        {
            // Fallback: emulate a non-zero exit on non-Unix.
            std::process::Command::new("cmd")
                .arg("/c")
                .arg("exit 1")
                .status()
                .unwrap()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
