//! Eval-set JSON model. Field shapes mirror the Python ADK `eval_set.py`
//! family for cross-tool interchange: ids serialize as `eval_set_id` /
//! `eval_id`, `final_response` is optional, and `intermediate_responses`
//! is a list of `(author, parts)` pairs — byte-compatible with eval sets
//! produced by `adk eval` in Python.

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::genai_types::{Content, Part};

/// One tool call captured during an invocation. Python stores full
/// `FunctionCall` objects here; extra fields (e.g. `id`) are ignored on
/// read and omitted on write.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolUse {
    /// Tool name.
    pub name: String,
    /// Args as JSON.
    #[serde(default)]
    pub args: Value,
}

/// Intermediate data captured during one [`Invocation`] (tool calls, etc).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct IntermediateData {
    /// Tool calls in execution order.
    #[serde(default)]
    pub tool_uses: Vec<ToolUse>,
    /// Intermediate model responses as `(author, parts)` pairs (Python:
    /// `list[tuple[str, list[Part]]]`, which serializes as JSON arrays).
    #[serde(default)]
    pub intermediate_responses: Vec<(String, Vec<Part>)>,
}

/// One user prompt → final response interaction.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Invocation {
    /// User prompt.
    pub user_content: Content,
    /// Expected (or actual) final response. Optional, like Python's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub final_response: Option<Content>,
    /// Intermediate data (tool calls and intermediate responses).
    #[serde(default)]
    pub intermediate_data: IntermediateData,
    /// Stable id for the invocation.
    #[serde(default)]
    pub invocation_id: String,
    /// Creation timestamp (seconds).
    #[serde(default)]
    pub creation_timestamp: f64,
}

/// Initial session fixture for a case (Python `SessionInput`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SessionInput {
    /// App name the session belongs to.
    #[serde(default)]
    pub app_name: String,
    /// User id the session belongs to.
    #[serde(default)]
    pub user_id: String,
    /// Initial session state.
    #[serde(default)]
    pub state: IndexMap<String, Value>,
}

/// One eval case: a sequence of invocations.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EvalCase {
    /// Stable id. Serializes as `eval_id` (Python ADK); `id` is accepted
    /// on read for files written by older adk-rs versions.
    #[serde(rename = "eval_id", alias = "id")]
    pub id: String,
    /// Conversation as a list of expected invocations.
    pub conversation: Vec<Invocation>,
    /// Optional initial session state.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_input: Option<SessionInput>,
    /// Optional human-readable name (adk-rs extension; Python ignores it).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Creation timestamp (seconds).
    #[serde(default)]
    pub creation_timestamp: f64,
}

/// A collection of eval cases.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EvalSet {
    /// Stable id. Serializes as `eval_set_id` (Python ADK); `id` is
    /// accepted on read for files written by older adk-rs versions.
    #[serde(rename = "eval_set_id", alias = "id")]
    pub id: String,
    /// Display name.
    #[serde(default)]
    pub name: String,
    /// Optional description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Eval cases.
    pub eval_cases: Vec<EvalCase>,
    /// Creation timestamp (seconds).
    #[serde(default)]
    pub creation_timestamp: f64,
}

/// Evaluation status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum EvalStatus {
    /// Score met or exceeded threshold.
    Passed,
    /// Score below threshold.
    Failed,
    /// Could not compute (missing data, etc).
    Error,
}

/// Result of one evaluator on one invocation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EvalScore {
    /// Score in [0, 1].
    pub score: f64,
    /// Pass/fail/error.
    pub status: EvalStatus,
    /// Free-form details.
    #[serde(default)]
    pub details: Value,
}

/// Aggregated result across all evaluators for one case.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EvalResult {
    /// Eval set id.
    pub eval_set_id: String,
    /// Eval case id.
    pub eval_case_id: String,
    /// Per-evaluator scores, keyed by evaluator name.
    pub scores: indexmap::IndexMap<String, EvalScore>,
    /// Overall pass/fail (logical AND of all individual statuses).
    pub overall_status: EvalStatus,
}
