//! Evaluation framework for adk-rs.
//!
//! Eval-set JSON IO is intentionally compatible with Python ADK's
//! `evaluation/eval_set.py`. Metrics included in v0.1: tool-trajectory
//! exact-match and a Rouge-L-ish text-match.

mod metrics;
mod runner;
mod set;

pub use metrics::{Evaluator, ResponseMatch, TrajectoryMatch};
pub use runner::{EvalReport, EvalRunner, load_eval_set_from_file, load_eval_set_from_str};
pub use set::{
    EvalCase, EvalResult, EvalScore, EvalSet, EvalStatus, IntermediateData, Invocation, ToolUse,
};
