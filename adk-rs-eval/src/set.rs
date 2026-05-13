//! Eval-set JSON model. Field shapes mirror the Python ADK `eval_set.py`
//! family for cross-tool interchange.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use adk_rs_genai_types::Content;

/// One tool call captured during an invocation.
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
    /// Intermediate model responses (e.g. tool-call assistant turns).
    #[serde(default)]
    pub intermediate_responses: Vec<Content>,
}

/// One user prompt → final response interaction.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Invocation {
    /// User prompt.
    pub user_content: Content,
    /// Expected (or actual) final response.
    pub final_response: Content,
    /// Intermediate data (tool calls and intermediate responses).
    #[serde(default)]
    pub intermediate_data: IntermediateData,
    /// Stable id for the invocation.
    #[serde(default)]
    pub invocation_id: String,
}

/// One eval case: a sequence of invocations.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EvalCase {
    /// Stable id.
    pub id: String,
    /// Conversation as a list of expected invocations.
    pub conversation: Vec<Invocation>,
    /// Optional initial session state.
    #[serde(default)]
    pub session_input: Option<Value>,
    /// Optional human-readable name.
    #[serde(default)]
    pub name: Option<String>,
}

/// A collection of eval cases.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EvalSet {
    /// Stable id.
    pub id: String,
    /// Display name.
    #[serde(default)]
    pub name: String,
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
