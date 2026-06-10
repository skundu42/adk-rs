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
        // One session per case, seeded with the case's `session_input`
        // fixture state; user messages replay in order.
        let initial_state = case
            .session_input
            .as_ref()
            .filter(|si| !si.state.is_empty())
            .map(|si| crate::core::State::from_iter(si.state.clone()));
        let session = svc
            .create_session(&self.app_name, &self.user_id, initial_state, None)
            .await?;
        let session = Arc::new(Mutex::new(session));

        // Per-evaluator scores, one entry per invocation; averaged below.
        let mut per_evaluator: IndexMap<String, Vec<EvalScore>> = IndexMap::new();

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
                root_agent: Some(self.agent.clone()),
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
            let mut actual_response: Option<crate::genai_types::Content> = None;
            let mut tool_uses: Vec<ToolUse> = Vec::new();
            let mut intermediate: Vec<(String, Vec<Part>)> = Vec::new();
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
                        actual_response = Some(c.clone());
                    } else {
                        intermediate.push((ev.author.clone(), c.parts.clone()));
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
                creation_timestamp: 0.0,
            };

            for evaluator in &self.evaluators {
                let score = evaluator.evaluate(inv, &actual).await?;
                per_evaluator
                    .entry(evaluator.name().to_string())
                    .or_default()
                    .push(score);
            }
        }

        // Aggregate: the reported score is the average across invocations
        // (the metric names say `avg`); status is the AND of per-invocation
        // statuses, with `Error` dominating. Per-invocation scores ride in
        // `details` for inspection.
        let mut scores: IndexMap<String, EvalScore> = IndexMap::new();
        let mut overall = EvalStatus::Passed;
        for (name, list) in per_evaluator {
            let n = list.len().max(1);
            let avg = list.iter().map(|s| s.score).sum::<f64>() / n as f64;
            let status = if list.iter().any(|s| s.status == EvalStatus::Error) {
                EvalStatus::Error
            } else if list.iter().all(|s| s.status == EvalStatus::Passed) {
                EvalStatus::Passed
            } else {
                EvalStatus::Failed
            };
            if status != EvalStatus::Passed {
                overall = EvalStatus::Failed;
            }
            let per_invocation: Vec<f64> = list.iter().map(|s| s.score).collect();
            scores.insert(
                name,
                EvalScore {
                    score: avg,
                    status,
                    details: serde_json::json!({ "per_invocation": per_invocation }),
                },
            );
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
            description: None,
            eval_cases: vec![EvalCase {
                id: "c1".into(),
                conversation: vec![Invocation {
                    user_content: crate::genai_types::Content::user_text("hi"),
                    final_response: Some(crate::genai_types::Content::model_text("hello world")),
                    intermediate_data: IntermediateData::default(),
                    invocation_id: String::new(),
                    creation_timestamp: 0.0,
                }],
                session_input: None,
                name: None,
                creation_timestamp: 0.0,
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
            description: None,
            eval_cases: vec![],
            creation_timestamp: 0.0,
        };
        let j = serde_json::to_string(&set).unwrap();
        // Ids hit the wire under their Python ADK names.
        assert!(j.contains("\"eval_set_id\""));
        let back = load_eval_set_from_str(&j).unwrap();
        assert_eq!(set, back);
    }

    /// Wire-compat fixture: this JSON matches what Python ADK's `adk eval`
    /// tooling writes (`eval_set_id`/`eval_id` keys, optional
    /// `final_response`, FunctionCall-shaped `tool_uses` with an `id`,
    /// tuple-shaped `intermediate_responses`, `session_input`). It must
    /// load without modification.
    #[tokio::test]
    async fn loads_python_adk_eval_set_fixture() {
        let fixture = r#"{
          "eval_set_id": "home_automation_agent_light_on_off_set",
          "name": "",
          "eval_cases": [
            {
              "eval_id": "eval_case_id",
              "conversation": [
                {
                  "invocation_id": "b7982664-0ab6-47cc-ab13-326656afdf75",
                  "user_content": {
                    "parts": [{"text": "Turn off device_2 in the Bedroom."}],
                    "role": "user"
                  },
                  "final_response": {
                    "parts": [{"text": "I have set the device_2 status to off."}],
                    "role": "model"
                  },
                  "intermediate_data": {
                    "tool_uses": [
                      {
                        "id": "adk-3964c554-8224-4910-b27d-b552c2f6da38",
                        "args": {"location": "Bedroom", "device_id": "device_2", "status": "OFF"},
                        "name": "set_device_info"
                      }
                    ],
                    "intermediate_responses": [
                      ["device_agent", [{"text": "setting device status"}]]
                    ]
                  },
                  "creation_timestamp": 1733760929.673543
                },
                {
                  "invocation_id": "9d2d2f4a-1111-4444-9999-2f2f2f2f2f2f",
                  "user_content": {
                    "parts": [{"text": "thanks"}],
                    "role": "user"
                  },
                  "final_response": null,
                  "creation_timestamp": 1733760930.5
                }
              ],
              "session_input": {
                "app_name": "home_automation_agent",
                "user_id": "test_user",
                "state": {"device_2": "on"}
              },
              "creation_timestamp": 1733760929.673546
            }
          ],
          "creation_timestamp": 1733760929.673552
        }"#;
        let set = load_eval_set_from_str(fixture).unwrap();
        assert_eq!(set.id, "home_automation_agent_light_on_off_set");
        let case = &set.eval_cases[0];
        assert_eq!(case.id, "eval_case_id");
        let inv = &case.conversation[0];
        assert_eq!(
            inv.final_response.as_ref().unwrap().text_concat(),
            "I have set the device_2 status to off."
        );
        assert_eq!(inv.intermediate_data.tool_uses[0].name, "set_device_info");
        assert_eq!(
            inv.intermediate_data.intermediate_responses[0].0,
            "device_agent"
        );
        assert!(case.conversation[1].final_response.is_none());
        let si = case.session_input.as_ref().unwrap();
        assert_eq!(si.app_name, "home_automation_agent");
        assert_eq!(si.state.get("device_2"), Some(&serde_json::json!("on")));
    }

    /// Multi-turn cases report the average across invocations, not just
    /// the last turn's score (the old behaviour overwrote per-turn scores).
    #[tokio::test]
    async fn multi_turn_scores_are_averaged() {
        // Agent answers "hello world" both turns; expectations match only
        // on the first turn → avg 0.5, overall Failed.
        let m = Arc::new(MockModel::new("mock"));
        m.push_text("hello world");
        m.push_text("hello world");
        let agent: Arc<dyn BaseAgent> = Arc::new(
            LlmAgent::builder("a")
                .model(m.clone() as Arc<dyn Model>)
                .build()
                .unwrap(),
        );
        let runner = EvalRunner::new(
            agent,
            "app",
            "u",
            vec![Arc::new(crate::eval::metrics::ResponseMatch::new(0.9))],
        );
        let mk_inv = |expected: &str| Invocation {
            user_content: crate::genai_types::Content::user_text("q"),
            final_response: Some(crate::genai_types::Content::model_text(expected)),
            intermediate_data: IntermediateData::default(),
            invocation_id: String::new(),
            creation_timestamp: 0.0,
        };
        let case = EvalCase {
            id: "c".into(),
            conversation: vec![mk_inv("hello world"), mk_inv("completely different")],
            session_input: None,
            name: None,
            creation_timestamp: 0.0,
        };
        let result = runner.run_case("s", &case).await.unwrap();
        let score = &result.scores["final_response_match_v1"];
        assert!((score.score - 0.5).abs() < 1e-9, "got {}", score.score);
        assert_eq!(score.status, EvalStatus::Failed);
        assert_eq!(result.overall_status, EvalStatus::Failed);
        assert_eq!(
            score.details["per_invocation"],
            serde_json::json!([1.0, 0.0])
        );
    }
}
