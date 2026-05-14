//! [`LocalCodeExecutor`] — runs code in a local subprocess via
//! [`tokio::process`].
//!
//! **Security**: This is *not* a sandbox. The child process runs with the
//! parent's user/network/filesystem. Use [`crate::code_exec::docker`] or a
//! cloud executor when you need isolation.

use async_trait::async_trait;
use std::time::Duration;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use tokio::time::timeout;

use crate::code_exec::CodeExecutor;
use crate::code_exec::types::{CodeExecutionInput, CodeExecutionResult};
use crate::core::InvocationContext;
use crate::error::{Error, Result};

/// Local-subprocess executor. `python3` by default; override via
/// [`LocalCodeExecutor::with_interpreter`].
#[derive(Debug, Clone)]
pub struct LocalCodeExecutor {
    interpreter: String,
    args: Vec<String>,
    timeout: Duration,
    retry_attempts: u32,
}

impl Default for LocalCodeExecutor {
    fn default() -> Self {
        Self {
            interpreter: "python3".into(),
            args: vec!["-".into()], // read code from stdin
            timeout: Duration::from_secs(30),
            retry_attempts: 2,
        }
    }
}

impl LocalCodeExecutor {
    /// Construct with defaults (`python3 -`, 30s timeout, 2 retries).
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Override the interpreter binary (e.g. `"node"`, `"bash"`).
    #[must_use]
    pub fn with_interpreter(mut self, interpreter: impl Into<String>) -> Self {
        self.interpreter = interpreter.into();
        self
    }

    /// Override the args passed to the interpreter. Use `-` so the child
    /// reads source from stdin (matches the default).
    #[must_use]
    pub fn with_args(mut self, args: Vec<String>) -> Self {
        self.args = args;
        self
    }

    /// Override the per-call wall-clock timeout.
    #[must_use]
    pub fn with_timeout(mut self, t: Duration) -> Self {
        self.timeout = t;
        self
    }
}

#[async_trait]
impl CodeExecutor for LocalCodeExecutor {
    fn timeout(&self) -> Option<Duration> {
        Some(self.timeout)
    }

    fn error_retry_attempts(&self) -> u32 {
        self.retry_attempts
    }

    async fn execute_code(
        &self,
        _ctx: &InvocationContext,
        input: CodeExecutionInput,
    ) -> Result<CodeExecutionResult> {
        let mut cmd = Command::new(&self.interpreter);
        cmd.args(&self.args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);
        let mut child = cmd
            .spawn()
            .map_err(|e| Error::other(format!("spawn {}: {e}", self.interpreter)))?;

        if let Some(mut stdin) = child.stdin.take() {
            // Swallow `BrokenPipe` — the child may exit before we finish
            // writing (e.g. syntax error on the first line). Real I/O
            // failures still propagate.
            if let Err(e) = stdin.write_all(input.code.as_bytes()).await {
                if e.kind() != std::io::ErrorKind::BrokenPipe {
                    return Err(Error::other(format!("write stdin: {e}")));
                }
            }
            // Drop stdin so the child sees EOF.
            drop(stdin);
        }

        let wait = async {
            child
                .wait_with_output()
                .await
                .map_err(|e| Error::other(format!("wait: {e}")))
        };

        let output = match timeout(self.timeout, wait).await {
            Ok(r) => r?,
            Err(_) => {
                // Process is dropped here → kill_on_drop fires.
                return Ok(CodeExecutionResult {
                    stdout: String::new(),
                    stderr: format!(
                        "{} execution timed out after {}s",
                        self.interpreter,
                        self.timeout.as_secs()
                    ),
                    output_files: Vec::new(),
                    exit_code: None,
                });
            }
        };

        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        Ok(CodeExecutionResult {
            stdout,
            stderr,
            output_files: Vec::new(),
            exit_code: output.status.code(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::tests_support::test_ctx;

    fn echo_executor() -> LocalCodeExecutor {
        // Use /bin/sh -s as a portable alternative when python3 may not be
        // installed. -s reads commands from stdin.
        LocalCodeExecutor::new()
            .with_interpreter("/bin/sh")
            .with_args(vec!["-s".into()])
    }

    #[tokio::test]
    async fn hello_world_round_trip() {
        let ex = echo_executor();
        let ctx = test_ctx();
        let out = ex
            .execute_code(
                &ctx,
                CodeExecutionInput {
                    code: "echo hello\necho err 1>&2\n".into(),
                    language: "shell".into(),
                    ..CodeExecutionInput::default()
                },
            )
            .await
            .unwrap();
        assert!(
            out.stdout.contains("hello"),
            "expected stdout to contain hello, got {out:?}"
        );
        assert!(
            out.stderr.contains("err"),
            "expected stderr to contain err, got {out:?}"
        );
    }

    #[tokio::test]
    async fn timeout_kills_runaway_process() {
        let ex = echo_executor().with_timeout(Duration::from_millis(150));
        let ctx = test_ctx();
        let out = ex
            .execute_code(
                &ctx,
                CodeExecutionInput {
                    code: "exec sleep 10\n".into(),
                    language: "shell".into(),
                    ..CodeExecutionInput::default()
                },
            )
            .await
            .unwrap();
        assert!(out.stderr.contains("timed out"));
    }
}
