//! [`ContainerCodeExecutor`] — runs code in a fresh ephemeral Docker
//! container per call.
//!
//! Spawns `docker run -i --rm --network=none --read-only ...` via
//! [`tokio::process`]. No bollard / Docker SDK dependency — just requires the
//! `docker` CLI on `$PATH`. For the trade-off of a few more milliseconds of
//! per-call CLI overhead we get a much smaller dep tree and immunity to
//! Docker daemon-protocol churn.
//!
//! The container is locked down:
//! - `--network=none` — no outbound network
//! - `--read-only` root filesystem
//! - `--rm` — auto-deletes on exit
//! - explicit `--name <id>` so the daemon-side container can be killed even
//!   if the parent `docker` CLI dies mid-run
//! - SIGKILL'd via `docker kill <id>` on wall-clock timeout (the daemon owns
//!   the container, not the CLI — `kill_on_drop(true)` on the CLI handle is
//!   not sufficient on its own)

use async_trait::async_trait;
use std::time::Duration;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use tokio::time::timeout;
use uuid::Uuid;

use crate::code_exec::CodeExecutor;
use crate::code_exec::types::{CodeExecutionInput, CodeExecutionResult};
use crate::core::InvocationContext;
use crate::error::{Error, Result};

/// Docker-backed sandbox executor. The container runs with network disabled
/// and a read-only root filesystem.
#[derive(Debug, Clone)]
pub struct ContainerCodeExecutor {
    /// Image tag, e.g. `"python:3.12-slim"`.
    pub image: String,
    /// Per-call wall-clock timeout.
    pub timeout: Duration,
    /// The argv after the image; `"{{code}}"` is replaced with the source.
    /// Default for Python: `["python3", "-"]` (reads code from stdin).
    pub argv: Vec<String>,
}

impl Default for ContainerCodeExecutor {
    fn default() -> Self {
        Self {
            image: "python:3.12-slim".into(),
            timeout: Duration::from_secs(30),
            argv: vec!["python3".into(), "-".into()],
        }
    }
}

impl ContainerCodeExecutor {
    /// New executor pinned to `image`.
    #[must_use]
    pub fn new(image: impl Into<String>) -> Self {
        Self {
            image: image.into(),
            ..Self::default()
        }
    }

    /// Override the per-call timeout.
    #[must_use]
    pub fn with_timeout(mut self, t: Duration) -> Self {
        self.timeout = t;
        self
    }

    /// Override the container argv.
    #[must_use]
    pub fn with_argv(mut self, argv: Vec<String>) -> Self {
        self.argv = argv;
        self
    }
}

/// Best-effort cleanup: kill and remove the named container. Used on timeout
/// (and as a defence-in-depth in case `--rm` didn't fire). Failures are
/// swallowed — we've already returned a timeout error to the caller and the
/// kill is opportunistic.
async fn kill_container(name: &str) {
    let _ = Command::new("docker").args(["kill", name]).output().await;
    let _ = Command::new("docker")
        .args(["rm", "-f", name])
        .output()
        .await;
}

#[async_trait]
impl CodeExecutor for ContainerCodeExecutor {
    fn timeout(&self) -> Option<Duration> {
        Some(self.timeout)
    }

    async fn execute_code(
        &self,
        _ctx: &InvocationContext,
        input: CodeExecutionInput,
    ) -> Result<CodeExecutionResult> {
        // Generate a stable name so the daemon-side container can be killed
        // on timeout. The `docker` CLI's `kill_on_drop` only kills the CLI
        // process — not the container.
        let container_name = format!("adk-rs-codex-{}", Uuid::new_v4());

        let mut cmd = Command::new("docker");
        cmd.arg("run")
            .arg("--rm")
            .arg("-i")
            .arg("--network=none")
            .arg("--read-only")
            .arg("--tmpfs=/tmp:rw,exec,size=64m")
            .arg("--name")
            .arg(&container_name)
            .arg(&self.image);
        for a in &self.argv {
            cmd.arg(a);
        }
        cmd.stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);

        let mut child = cmd
            .spawn()
            .map_err(|e| Error::other(format!("docker run spawn: {e}")))?;
        if let Some(mut stdin) = child.stdin.take() {
            // Swallow `BrokenPipe` — the child may exit before consuming all
            // input (e.g. parse error on the first line). We still want to
            // read whatever stderr/stdout it produced.
            if let Err(e) = stdin.write_all(input.code.as_bytes()).await {
                if e.kind() != std::io::ErrorKind::BrokenPipe {
                    // Stop and clean up on a real I/O error.
                    kill_container(&container_name).await;
                    return Err(Error::other(format!("docker stdin: {e}")));
                }
            }
            drop(stdin);
        }

        let wait = async {
            child
                .wait_with_output()
                .await
                .map_err(|e| Error::other(format!("docker wait: {e}")))
        };
        let output = match timeout(self.timeout, wait).await {
            Ok(r) => r?,
            Err(_) => {
                // Daemon-side container is still running. Kill it explicitly.
                kill_container(&container_name).await;
                return Ok(CodeExecutionResult {
                    stdout: String::new(),
                    stderr: format!(
                        "container '{container_name}' execution timed out after {}s",
                        self.timeout.as_secs()
                    ),
                    output_files: Vec::new(),
                    exit_code: None,
                });
            }
        };
        Ok(CodeExecutionResult {
            stdout: String::from_utf8_lossy(&output.stdout).to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
            output_files: Vec::new(),
            exit_code: output.status.code(),
        })
    }
}
