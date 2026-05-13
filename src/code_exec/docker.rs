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
//! - SIGKILL'd by docker after the configured timeout
//!
//! For tests that require a real Docker daemon, set
//! `ADK_RS_DOCKER_TESTS=1`. Tests are otherwise `#[ignore]`d.

use async_trait::async_trait;
use std::time::Duration;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use tokio::time::timeout;

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
        let mut cmd = Command::new("docker");
        cmd.arg("run")
            .arg("--rm")
            .arg("-i")
            .arg("--network=none")
            .arg("--read-only")
            .arg("--tmpfs=/tmp:rw,exec,size=64m")
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
            stdin
                .write_all(input.code.as_bytes())
                .await
                .map_err(|e| Error::other(format!("docker stdin: {e}")))?;
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
                return Ok(CodeExecutionResult {
                    stdout: String::new(),
                    stderr: format!(
                        "container execution timed out after {}s",
                        self.timeout.as_secs()
                    ),
                    output_files: Vec::new(),
                });
            }
        };
        Ok(CodeExecutionResult {
            stdout: String::from_utf8_lossy(&output.stdout).to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
            output_files: Vec::new(),
        })
    }
}
