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

use std::process::Stdio;

use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;

use crate::security::sanitize;

/// Events emitted during a process run. Text payloads are already sanitized.
#[derive(Debug, Clone)]
pub enum ProcessEvent {
    Started {
        pid: Option<u32>,
    },
    Stdout(String),
    Stderr(String),
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

/// Spawn `binary` with `args`. Returns immediately with a [`RunHandle`]; the
/// process is driven on a background thread.
pub fn spawn(binary: impl Into<String>, args: Vec<String>) -> RunHandle {
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
        rt.block_on(drive(binary, args, ev_tx, cancel_rx));
    });

    RunHandle {
        events: ev_rx,
        cancel: cancel_tx,
    }
}

async fn drive(
    binary: String,
    args: Vec<String>,
    ev: async_channel::Sender<ProcessEvent>,
    cancel_rx: async_channel::Receiver<()>,
) {
    let mut child = match Command::new(&binary)
        .args(&args)
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
    let read_stdout = async move {
        let mut lines = BufReader::new(stdout).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            let _ = ev_out
                .send(ProcessEvent::Stdout(sanitize::redact(&line)))
                .await;
        }
    };

    let ev_err = ev.clone();
    let read_stderr = async move {
        let mut lines = BufReader::new(stderr).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            let _ = ev_err
                .send(ProcessEvent::Stderr(sanitize::redact(&line)))
                .await;
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
                return status.map(|_| std::process::ExitStatus::default_failed());
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
