//! Built-in metrics: tool-trajectory exact match and Rouge-L-ish text match.

use async_trait::async_trait;

use crate::error::Result;

use crate::eval::set::{EvalScore, EvalStatus, Invocation};

/// Common evaluator shape.
#[async_trait]
pub trait Evaluator: Send + Sync + 'static {
    /// Stable name used as the metric key.
    fn name(&self) -> &str;

    /// Score one invocation pair.
    async fn evaluate(&self, expected: &Invocation, actual: &Invocation) -> Result<EvalScore>;
}

/// Exact-match (in-order) trajectory evaluator. Matches `(tool_name, args)`
/// pairs from `intermediate_data.tool_uses`. Score = matched / max(expected, actual).
#[derive(Debug, Default)]
pub struct TrajectoryMatch {
    threshold: f64,
}

impl TrajectoryMatch {
    /// Construct with the given pass threshold (default 1.0 = exact match).
    #[must_use]
    pub fn new(threshold: f64) -> Self {
        Self { threshold }
    }
}

#[async_trait]
impl Evaluator for TrajectoryMatch {
    fn name(&self) -> &str {
        "tool_trajectory_avg_score"
    }
    async fn evaluate(&self, expected: &Invocation, actual: &Invocation) -> Result<EvalScore> {
        let e = &expected.intermediate_data.tool_uses;
        let a = &actual.intermediate_data.tool_uses;
        let denom = e.len().max(a.len()).max(1);
        let mut matched = 0;
        for (i, ex) in e.iter().enumerate() {
            if let Some(ac) = a.get(i) {
                if ex.name == ac.name && ex.args == ac.args {
                    matched += 1;
                }
            }
        }
        let score = (matched as f64) / (denom as f64);
        let status = if score + 1e-9 >= self.threshold {
            EvalStatus::Passed
        } else {
            EvalStatus::Failed
        };
        Ok(EvalScore {
            score,
            status,
            details: serde_json::json!({"matched": matched, "expected": e.len(), "actual": a.len()}),
        })
    }
}

/// Rough text-overlap metric: ratio of expected unigrams that appear in the
/// actual response (case-insensitive). Not a true Rouge-L; sufficient for
/// regression tests in v0.1.
#[derive(Debug, Default)]
pub struct ResponseMatch {
    threshold: f64,
}

impl ResponseMatch {
    /// Construct.
    #[must_use]
    pub fn new(threshold: f64) -> Self {
        Self { threshold }
    }
}

#[async_trait]
impl Evaluator for ResponseMatch {
    fn name(&self) -> &str {
        "final_response_match_v1"
    }
    async fn evaluate(&self, expected: &Invocation, actual: &Invocation) -> Result<EvalScore> {
        let e = expected.final_response.text_concat().to_lowercase();
        let a = actual.final_response.text_concat().to_lowercase();
        let e_tokens: Vec<&str> = e.split_whitespace().collect();
        if e_tokens.is_empty() {
            return Ok(EvalScore {
                score: 1.0,
                status: EvalStatus::Passed,
                details: serde_json::json!({"reason": "empty expected"}),
            });
        }
        let mut hit = 0;
        for t in &e_tokens {
            if a.contains(t) {
                hit += 1;
            }
        }
        let score = (hit as f64) / (e_tokens.len() as f64);
        let status = if score + 1e-9 >= self.threshold {
            EvalStatus::Passed
        } else {
            EvalStatus::Failed
        };
        Ok(EvalScore {
            score,
            status,
            details: serde_json::json!({
                "expected_tokens": e_tokens.len(),
                "hit": hit,
            }),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::eval::set::{IntermediateData, ToolUse};
    use crate::genai_types::Content;

    #[tokio::test]
    async fn trajectory_exact_match() {
        let m = TrajectoryMatch::new(1.0);
        let e = Invocation {
            user_content: Content::user_text(""),
            final_response: Content::model_text(""),
            intermediate_data: IntermediateData {
                tool_uses: vec![ToolUse {
                    name: "f".into(),
                    args: serde_json::json!({"x": 1}),
                }],
                ..Default::default()
            },
            invocation_id: String::new(),
        };
        let r = m.evaluate(&e, &e).await.unwrap();
        assert!((r.score - 1.0).abs() < 1e-9);
        assert_eq!(r.status, EvalStatus::Passed);
    }

    #[tokio::test]
    async fn response_match_substring_score() {
        let m = ResponseMatch::new(0.5);
        let e = Invocation {
            user_content: Content::user_text(""),
            final_response: Content::model_text("hello world"),
            intermediate_data: IntermediateData::default(),
            invocation_id: String::new(),
        };
        let a = Invocation {
            user_content: Content::user_text(""),
            final_response: Content::model_text("Why, hello there"),
            intermediate_data: IntermediateData::default(),
            invocation_id: String::new(),
        };
        let r = m.evaluate(&e, &a).await.unwrap();
        // 1 of 2 expected tokens ("hello") found.
        assert!((r.score - 0.5).abs() < 1e-9);
        assert_eq!(r.status, EvalStatus::Passed);
    }
}
