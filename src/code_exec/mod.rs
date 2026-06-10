//! Code-execution subsystem (`feature = "code-exec"`).
//!
//! Lets agents emit `ExecutableCode` parts and have the runtime run the
//! code in a sandbox of varying degrees of safety.
//!
//! - [`local::LocalCodeExecutor`] — spawns a child interpreter via
//!   [`tokio::process`] with a timeout. Local subprocess isolation only —
//!   *not* a security boundary.
//! - [`docker::ContainerCodeExecutor`] (`feature = "code-exec-docker"`) —
//!   spins up an ephemeral Docker container per call.
//!
//! The runner extracts `ExecutableCode` parts from model responses, invokes
//! the agent's executor, and feeds back `CodeExecutionResult` parts. Attach
//! an executor via `LlmAgent::builder().code_executor(...)`.

mod types;

pub mod local;

#[cfg(feature = "code-exec-docker")]
pub mod docker;

pub use types::{CodeExecutionInput, CodeExecutionResult, ExecFile};

use async_trait::async_trait;
use std::time::Duration;

use crate::core::InvocationContext;
use crate::error::Result;

/// Pluggable executor for `ExecutableCode` parts.
#[async_trait]
pub trait CodeExecutor: Send + Sync + std::fmt::Debug + 'static {
    /// Whether the executor maintains interpreter state across calls (e.g.
    /// notebook-style executors). When `true`, the runner threads an
    /// `execution_id` into each input.
    fn stateful(&self) -> bool {
        false
    }

    /// Total number of attempts (minimum 1) the runner makes per execution
    /// before giving up — the default of 2 means one retry.
    fn error_retry_attempts(&self) -> u32 {
        2
    }

    /// Per-invocation wall-clock timeout.
    fn timeout(&self) -> Option<Duration> {
        Some(Duration::from_secs(30))
    }

    /// Execute one block of code.
    async fn execute_code(
        &self,
        ctx: &InvocationContext,
        input: CodeExecutionInput,
    ) -> Result<CodeExecutionResult>;
}
