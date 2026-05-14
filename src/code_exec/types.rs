//! Data shapes for code execution inputs and outputs.

use serde::{Deserialize, Serialize};

/// One file passed to or returned from an executor.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecFile {
    /// File name (no directory).
    pub name: String,
    /// Raw bytes.
    #[serde(with = "serde_bytes")]
    pub content: Vec<u8>,
    /// MIME type guess.
    #[serde(default)]
    pub mime_type: Option<String>,
}

mod serde_bytes {
    use base64::Engine;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(bytes: &[u8], s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&base64::engine::general_purpose::STANDARD.encode(bytes))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
        use serde::de::Error;
        let s: String = String::deserialize(d)?;
        base64::engine::general_purpose::STANDARD
            .decode(s.as_bytes())
            .map_err(D::Error::custom)
    }
}

/// Inputs to one execution.
#[derive(Debug, Clone, Default)]
pub struct CodeExecutionInput {
    /// Source to run.
    pub code: String,
    /// Lowercase language id (`"python"`, `"shell"`, ...).
    pub language: String,
    /// Files to stage before running.
    pub input_files: Vec<ExecFile>,
    /// Stable id for stateful executors. `None` for stateless.
    pub execution_id: Option<String>,
}

/// Output of one execution.
#[derive(Debug, Clone, Default)]
pub struct CodeExecutionResult {
    /// stdout produced.
    pub stdout: String,
    /// stderr produced.
    pub stderr: String,
    /// Files produced (e.g. plots written to disk).
    pub output_files: Vec<ExecFile>,
    /// Process exit code, when one is available. `None` means the executor
    /// could not determine an exit code (e.g. the process was killed by the
    /// timeout watchdog before it had a chance to exit). Callers should
    /// treat `Some(0)` as success, `Some(_)` as failure, and `None` as
    /// "unknown — inspect `stderr`".
    pub exit_code: Option<i32>,
}

impl CodeExecutionResult {
    /// Concatenate stdout + stderr into a single string for surfacing to
    /// the model as a `CodeExecutionResult` part.
    #[must_use]
    pub fn combined_output(&self) -> String {
        if self.stderr.is_empty() {
            self.stdout.clone()
        } else if self.stdout.is_empty() {
            self.stderr.clone()
        } else {
            format!("{}\n--- stderr ---\n{}", self.stdout, self.stderr)
        }
    }

    /// True when the executor reports a clean exit (`exit_code == Some(0)`)
    /// **or** when no exit code is available and `stderr` is empty.
    /// Routing of `Outcome::OutcomeOk`/`OutcomeFailed` should use this rather
    /// than the v0.2-initial `stderr.is_empty()` heuristic.
    #[must_use]
    pub fn is_success(&self) -> bool {
        match self.exit_code {
            Some(0) => true,
            Some(_) => false,
            None => self.stderr.is_empty(),
        }
    }
}
