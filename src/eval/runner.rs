//! Replays expected invocations through an agent and aggregates scores.

use std::sync::Arc;

use futures::StreamExt;
use indexmap::IndexMap;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

use crate::agents::BaseAgent;
use crate::core::{InvocationContext, InvocationOrigin, RunConfig, SessionService};
use crate::error::Result;
use crate::genai_types::Part;
use crate::services::mem::InMemorySessionService;

use crate::eval::metrics::Evaluator;
use crate::eval::set::{
    EvalCase, EvalResult, EvalScore, EvalSet, EvalStatus, IntermediateData, Invocation, ToolUse,
};

/// Top-level eval report.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvalReport {
    /// Per-case results, in eval-set order.
    pub results: Vec<EvalResult>,
}

/// Executes an [`EvalSet`] against a [`BaseAgent`].
pub struct EvalRunner {
    agent: Arc<dyn BaseAgent>,
    app_name: String,
    user_id: String,
    evaluators: Vec<Arc<dyn Evaluator>>,
}

impl std::fmt::Debug for EvalRunner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EvalRunner")
            .field("app_name", &self.app_name)
            .field("user_id", &self.user_id)
            .field("agent", &self.agent.name())
            .finish_non_exhaustive()
    }
}

impl EvalRunner {
    /// Construct.
    pub fn new(
        agent: Arc<dyn BaseAgent>,
        app_name: impl Into<String>,
        user_id: impl Into<String>,
        evaluators: Vec<Arc<dyn Evaluator>>,
    ) -> Self {
        Self {
            agent,
            app_name: app_name.into(),
            user_id: user_id.into(),
            evaluators,
        }
    }

    /// Run the entire eval set.
    pub async fn run_set(&self, set: &EvalSet) -> Result<EvalReport> {
        let mut results = Vec::with_capacity(set.eval_cases.len());
        for case in &set.eval_cases {
            results.push(self.run_case(&set.id, case).await?);
        }
        Ok(EvalReport { results })
    }

    /// Run a single case.
    pub async fn run_case(&self, set_id: &str, case: &EvalCase) -> Result<EvalResult> {
        let svc: Arc<dyn SessionService> = Arc::new(InMemorySessionService::new());
        // One session per case; user messages replay in order.
        let session = svc
            .create_session(&self.app_name, &self.user_id, None, None)
            .await?;
        let session = Arc::new(Mutex::new(session));

        let mut scores: IndexMap<String, EvalScore> = IndexMap::new();
        let mut overall = EvalStatus::Passed;

        for inv in &case.conversation {
            let ctx = Arc::new(InvocationContext {
                app_name: self.app_name.clone(),
                user_id: self.user_id.clone(),
                invocation_id: InvocationContext::new_id(),
                session: session.clone(),
                session_service: svc.clone(),
                artifact_service: None,
                memory_service: None,
                credential_service: None,
                run_config: RunConfig::default(),
                origin: InvocationOrigin::Api,
                user_content: Some(inv.user_content.clone()),
                llm_call_count: Arc::new(Mutex::new(0)),
                cancellation: Default::default(),
                attributes: Arc::new(Mutex::new(std::collections::HashMap::new())),
            });
            // Append the user content event.
            {
                let mut s = session.lock();
                s.events.push(crate::core::Event::new(
                    "user",
                    crate::core::LlmResponse {
                        content: Some(inv.user_content.clone()),
                        ..Default::default()
                    },
                ));
            }

            let mut stream = self.agent.clone().run(ctx.clone()).await?;
            let mut actual_response = crate::genai_types::Content::model_text("");
            let mut tool_uses: Vec<ToolUse> = Vec::new();
            let mut intermediate: Vec<crate::genai_types::Content> = Vec::new();
            while let Some(ev) = stream.next().await {
                let ev = ev?;
                if let Some(c) = &ev.response.content {
                    // Track function calls.
                    for p in &c.parts {
                        if let Part::FunctionCall(fc) = p {
                            tool_uses.push(ToolUse {
                                name: fc.name.clone(),
                                args: fc.args.clone(),
                            });
                        }
                    }
                    if ev.is_final_response() {
                        actual_response = c.clone();
                    } else {
                        intermediate.push(c.clone());
                    }
                }
            }
            let actual = Invocation {
                user_content: inv.user_content.clone(),
                final_response: actual_response,
                intermediate_data: IntermediateData {
                    tool_uses,
                    intermediate_responses: intermediate,
                },
                invocation_id: ctx.invocation_id.clone(),
            };

            for evaluator in &self.evaluators {
                let score = evaluator.evaluate(inv, &actual).await?;
                if score.status != EvalStatus::Passed {
                    overall = EvalStatus::Failed;
                }
                scores.insert(evaluator.name().to_string(), score);
            }
        }

        Ok(EvalResult {
            eval_set_id: set_id.to_string(),
            eval_case_id: case.id.clone(),
            scores,
            overall_status: overall,
        })
    }
}

/// Load an eval set from JSON.
pub fn load_eval_set_from_str(s: &str) -> Result<EvalSet> {
    Ok(serde_json::from_str(s)?)
}

/// Load an eval set from a JSON file.
pub async fn load_eval_set_from_file(path: impl AsRef<std::path::Path>) -> Result<EvalSet> {
    let bytes = tokio::fs::read(path).await?;
    Ok(serde_json::from_slice(&bytes)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::LlmAgent;
    use crate::core::Model;
    use crate::core::testing::MockModel;

    fn make_agent(text: &str) -> Arc<dyn BaseAgent> {
        let m = Arc::new(MockModel::new("mock"));
        m.push_text(text);
        Arc::new(
            LlmAgent::builder("a")
                .model(m.clone() as Arc<dyn Model>)
                .instruction("x")
                .build()
                .unwrap(),
        )
    }

    #[tokio::test]
    async fn eval_runs_response_match() {
        let agent = make_agent("hello world from agent");
        let runner = EvalRunner::new(
            agent,
            "app",
            "u",
            vec![Arc::new(crate::eval::metrics::ResponseMatch::new(0.5))],
        );
        let set = EvalSet {
            id: "s".into(),
            name: "demo".into(),
            eval_cases: vec![EvalCase {
                id: "c1".into(),
                conversation: vec![Invocation {
                    user_content: crate::genai_types::Content::user_text("hi"),
                    final_response: crate::genai_types::Content::model_text("hello world"),
                    intermediate_data: IntermediateData::default(),
                    invocation_id: String::new(),
                }],
                session_input: None,
                name: None,
            }],
            creation_timestamp: 0.0,
        };
        let report = runner.run_set(&set).await.unwrap();
        assert_eq!(report.results.len(), 1);
        assert_eq!(report.results[0].overall_status, EvalStatus::Passed);
    }

    #[tokio::test]
    async fn eval_set_round_trips_json() {
        let set = EvalSet {
            id: "s".into(),
            name: "demo".into(),
            eval_cases: vec![],
            creation_timestamp: 0.0,
        };
        let j = serde_json::to_string(&set).unwrap();
        let back = load_eval_set_from_str(&j).unwrap();
        assert_eq!(set, back);
    }
}
