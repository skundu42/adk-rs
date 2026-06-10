//! Legacy HTTP route handlers.
//!
//! The run/session surface moved to [`crate::server::adk_web`], which
//! implements Python ADK's `adk api_server` wire contract (use `/list-apps`,
//! `/run`, `/run_sse`, and `/apps/{app}/users/{user}/sessions/...`).
//! `/list-agents` is kept for existing adk-rs clients.

use axum::Json;
use axum::extract::State;
use serde::Serialize;

use crate::server::app::AppState;

#[derive(Debug, Serialize)]
pub(crate) struct AgentList {
    agents: Vec<String>,
}

pub(crate) async fn list_agents(State(state): State<AppState>) -> Json<AgentList> {
    Json(AgentList {
        agents: state.runners.keys().cloned().collect(),
    })
}
