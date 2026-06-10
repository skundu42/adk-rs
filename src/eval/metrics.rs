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
#[derive(Debug)]
pub struct TrajectoryMatch {
    threshold: f64,
}

impl Default for TrajectoryMatch {
    /// Exact match required (threshold 1.0). A 0.0 default would pass
    /// everything.
    fn default() -> Self {
        Self { threshold: 1.0 }
    }
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

/// Rough text-overlap metric: ratio of expected unigrams that appear as
/// whole tokens in the actual response (case-insensitive). Not a true
/// Rouge-L; sufficient for regression tests.
#[derive(Debug)]
pub struct ResponseMatch {
    threshold: f64,
}

impl Default for ResponseMatch {
    /// 0.8 token overlap required. A 0.0 default would pass everything.
    fn default() -> Self {
        Self { threshold: 0.8 }
    }
}

impl ResponseMatch {
    /// Construct.
    #[must_use]
    pub fn new(threshold: f64) -> Self {
        Self { threshold }
    }
}

fn response_text(c: &Option<crate::genai_types::Content>) -> String {
    c.as_ref().map(|c| c.text_concat()).unwrap_or_default()
}

#[async_trait]
impl Evaluator for ResponseMatch {
    fn name(&self) -> &str {
        "final_response_match_v1"
    }
    async fn evaluate(&self, expected: &Invocation, actual: &Invocation) -> Result<EvalScore> {
        let e = response_text(&expected.final_response).to_lowercase();
        let a = response_text(&actual.final_response).to_lowercase();
        let e_tokens: Vec<&str> = e.split_whitespace().collect();
        if e_tokens.is_empty() {
            return Ok(EvalScore {
                score: 1.0,
                status: EvalStatus::Passed,
                details: serde_json::json!({"reason": "empty expected"}),
            });
        }
        // Whole-token matching: substring containment would let "cat"
        // match "concatenate" and inflate scores.
        let a_tokens: std::collections::HashSet<&str> = a.split_whitespace().collect();
        let mut hit = 0;
        for t in &e_tokens {
            if a_tokens.contains(t) {
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

    fn inv(final_text: &str, tool_uses: Vec<ToolUse>) -> Invocation {
        Invocation {
            user_content: Content::user_text(""),
            final_response: Some(Content::model_text(final_text)),
            intermediate_data: IntermediateData {
                tool_uses,
                ..Default::default()
            },
            invocation_id: String::new(),
            creation_timestamp: 0.0,
        }
    }

    #[tokio::test]
    async fn trajectory_exact_match() {
        let m = TrajectoryMatch::new(1.0);
        let e = inv(
            "",
            vec![ToolUse {
                name: "f".into(),
                args: serde_json::json!({"x": 1}),
            }],
        );
        let r = m.evaluate(&e, &e).await.unwrap();
        assert!((r.score - 1.0).abs() < 1e-9);
        assert_eq!(r.status, EvalStatus::Passed);
    }

    #[tokio::test]
    async fn response_match_token_score() {
        let m = ResponseMatch::new(0.5);
        let e = inv("hello world", vec![]);
        let a = inv("Why, hello there", vec![]);
        let r = m.evaluate(&e, &a).await.unwrap();
        // 1 of 2 expected tokens ("hello") found.
        assert!((r.score - 0.5).abs() < 1e-9);
        assert_eq!(r.status, EvalStatus::Passed);
    }

    /// Regression: token matching is whole-token, not substring — "cat"
    /// must not match inside "concatenate".
    #[tokio::test]
    async fn response_match_rejects_substring_hits() {
        let m = ResponseMatch::new(0.5);
        let e = inv("cat", vec![]);
        let a = inv("concatenate strings", vec![]);
        let r = m.evaluate(&e, &a).await.unwrap();
        assert!((r.score - 0.0).abs() < 1e-9);
        assert_eq!(r.status, EvalStatus::Failed);
    }

    /// Defaults must not pass everything.
    #[tokio::test]
    async fn default_thresholds_are_strict() {
        let response_match = ResponseMatch::default();
        let expected = inv("alpha beta gamma delta epsilon", vec![]);
        let actual = inv("alpha", vec![]);
        let r = response_match.evaluate(&expected, &actual).await.unwrap();
        assert_eq!(r.status, EvalStatus::Failed);

        let trajectory = TrajectoryMatch::default();
        let expected = inv(
            "",
            vec![ToolUse {
                name: "f".into(),
                args: serde_json::json!({}),
            }],
        );
        let actual = inv("", vec![]);
        let r = trajectory.evaluate(&expected, &actual).await.unwrap();
        assert_eq!(r.status, EvalStatus::Failed);
    }
}
