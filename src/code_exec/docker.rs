//! [`ContainerCodeExecutor`] — runs code in a fresh ephemeral Docker
//! container per call.
//!
//! Spawns `docker run -i --rm ...` via [`tokio::process`]. No bollard /
//! Docker SDK dependency — just requires the `docker` CLI on `$PATH`. For
//! the trade-off of a few more milliseconds of per-call CLI overhead we get
//! a much smaller dep tree and immunity to Docker daemon-protocol churn.
//!
//! The container is locked down by default:
//! - `--network=none` — no outbound network
//! - `--read-only` root filesystem
//! - `--rm` — auto-deletes on exit
//! - `--memory` / `--memory-swap` — hard RSS cap (default 256 MiB) so
//!   runaway allocations get OOM-killed by the daemon, not the host
//! - `--cpus` — fractional CPU cap (default 1.0 core)
//! - `--pids-limit` — fork-bomb cap (default 128 PIDs)
//! - `--cap-drop=ALL` — drop every Linux capability
//! - `--security-opt=no-new-privileges` — block setuid/setgid escalation
//! - `--user 65534:65534` — run as the conventional `nobody` UID, never root
//! - explicit `--name <id>` so the daemon-side container can be killed even
//!   if the parent `docker` CLI dies mid-run
//! - SIGKILL'd via `docker kill <id>` on wall-clock timeout (the daemon owns
//!   the container, not the CLI — `kill_on_drop(true)` on the CLI handle is
//!   not sufficient on its own)
//!
//! All limits are configurable on [`ContainerCodeExecutor`] for workloads
//! that legitimately need more headroom. Removing them is a deliberate act.

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

/// Docker-backed sandbox executor. The container runs locked down by
/// default: no network, read-only rootfs, memory/CPU/pids caps, all
/// capabilities dropped, no-new-privileges, and a non-root user.
#[derive(Debug, Clone)]
pub struct ContainerCodeExecutor {
    /// Image tag, e.g. `"python:3.12-slim"`.
    pub image: String,
    /// Per-call wall-clock timeout.
    pub timeout: Duration,
    /// The argv after the image; `"{{code}}"` is replaced with the source.
    /// Default for Python: `["python3", "-"]` (reads code from stdin).
    pub argv: Vec<String>,
    /// Hard memory cap passed to `--memory` (e.g. `"256m"`, `"1g"`). Default
    /// `"256m"`. `--memory-swap` is set to the same value so swap-out can't
    /// be used to inflate the RSS ceiling.
    pub memory: String,
    /// Fractional CPU cap passed to `--cpus` (e.g. `"1.0"`, `"0.5"`).
    /// Default `"1.0"` (one core's worth of time per wall-clock second).
    pub cpus: String,
    /// Max number of processes the container may create (`--pids-limit`).
    /// Default `128`.
    pub pids_limit: u32,
    /// `--user` value (`uid` or `uid:gid`). Default `"65534:65534"` — the
    /// `nobody` user on most distros — to ensure the container never runs
    /// as root.
    pub user: String,
    /// If true (default), pass `--cap-drop=ALL` and
    /// `--security-opt=no-new-privileges`. Setting this to `false` is for
    /// debugging only and breaks the sandbox guarantee.
    pub drop_capabilities: bool,
    /// Extra `docker run` arguments to splice in *before* the image. Use
    /// sparingly — most legitimate needs are already exposed as typed
    /// fields above.
    pub extra_args: Vec<String>,
}

impl Default for ContainerCodeExecutor {
    fn default() -> Self {
        Self {
            image: "python:3.12-slim".into(),
            timeout: Duration::from_secs(30),
            argv: vec!["python3".into(), "-".into()],
            memory: "256m".into(),
            cpus: "1.0".into(),
            pids_limit: 128,
            user: "65534:65534".into(),
            drop_capabilities: true,
            extra_args: Vec::new(),
        }
    }
}

impl ContainerCodeExecutor {
    /// New executor pinned to `image`, with the secure defaults.
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

    /// Override the `--memory` cap.
    #[must_use]
    pub fn with_memory(mut self, memory: impl Into<String>) -> Self {
        self.memory = memory.into();
        self
    }

    /// Override the `--cpus` cap.
    #[must_use]
    pub fn with_cpus(mut self, cpus: impl Into<String>) -> Self {
        self.cpus = cpus.into();
        self
    }

    /// Override the `--pids-limit` cap.
    #[must_use]
    pub fn with_pids_limit(mut self, pids_limit: u32) -> Self {
        self.pids_limit = pids_limit;
        self
    }

    /// Override the `--user` value.
    #[must_use]
    pub fn with_user(mut self, user: impl Into<String>) -> Self {
        self.user = user.into();
        self
    }

    /// Append extra raw arguments to `docker run`. Inserted before the
    /// image. Reserved for advanced users; doesn't bypass the typed limits.
    #[must_use]
    pub fn with_extra_args(mut self, args: Vec<String>) -> Self {
        self.extra_args = args;
        self
    }

    /// Build the full `docker run ...` argv (without the leading `docker`
    /// program). Exposed so the policy can be inspected in tests.
    pub fn build_run_args(&self, container_name: &str) -> Vec<String> {
        let mut a: Vec<String> = vec![
            "run".into(),
            "--rm".into(),
            "-i".into(),
            "--network=none".into(),
            "--read-only".into(),
            "--tmpfs=/tmp:rw,exec,size=64m".into(),
            format!("--memory={}", self.memory),
            format!("--memory-swap={}", self.memory),
            format!("--cpus={}", self.cpus),
            format!("--pids-limit={}", self.pids_limit),
            format!("--user={}", self.user),
        ];
        if self.drop_capabilities {
            a.push("--cap-drop=ALL".into());
            a.push("--security-opt=no-new-privileges".into());
        }
        a.push("--name".into());
        a.push(container_name.to_string());
        a.extend(self.extra_args.iter().cloned());
        a.push(self.image.clone());
        a.extend(self.argv.iter().cloned());
        a
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
        cmd.args(self.build_run_args(&container_name))
            .stdin(std::process::Stdio::piped())
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_args_include_full_lockdown() {
        let ex = ContainerCodeExecutor::default();
        let args = ex.build_run_args("name-1");
        let joined = args.join(" ");
        // Network + filesystem isolation.
        assert!(args.iter().any(|a| a == "--network=none"));
        assert!(args.iter().any(|a| a == "--read-only"));
        // Resource caps.
        assert!(joined.contains("--memory=256m"));
        assert!(joined.contains("--memory-swap=256m"));
        assert!(joined.contains("--cpus=1.0"));
        assert!(joined.contains("--pids-limit=128"));
        // Capability + privilege drops.
        assert!(args.iter().any(|a| a == "--cap-drop=ALL"));
        assert!(args.iter().any(|a| a == "--security-opt=no-new-privileges"));
        // Non-root user.
        assert!(joined.contains("--user=65534:65534"));
        // Container name + image + argv ordering.
        let name_idx = args.iter().position(|a| a == "--name").unwrap();
        assert_eq!(args[name_idx + 1], "name-1");
        let image_idx = args.iter().position(|a| a == "python:3.12-slim").unwrap();
        assert!(image_idx > name_idx, "image must follow --name");
        assert_eq!(args.last().unwrap(), "-");
    }

    #[test]
    fn overrides_take_effect() {
        let ex = ContainerCodeExecutor::new("alpine")
            .with_memory("64m")
            .with_cpus("0.25")
            .with_pids_limit(32)
            .with_user("1000:1000");
        let args = ex.build_run_args("n").join(" ");
        assert!(args.contains("--memory=64m"));
        assert!(args.contains("--memory-swap=64m"));
        assert!(args.contains("--cpus=0.25"));
        assert!(args.contains("--pids-limit=32"));
        assert!(args.contains("--user=1000:1000"));
    }

    #[test]
    fn drop_capabilities_off_explicitly_omits_them() {
        // Toggling the field off is an explicit user act — verify the
        // resulting argv matches.
        let mut ex = ContainerCodeExecutor::default();
        ex.drop_capabilities = false;
        let args = ex.build_run_args("n");
        assert!(!args.iter().any(|a| a == "--cap-drop=ALL"));
        assert!(!args.iter().any(|a| a == "--security-opt=no-new-privileges"));
    }
}
