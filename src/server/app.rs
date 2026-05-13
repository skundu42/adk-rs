//! Server bootstrap.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;

use axum::Router;
use axum::routing::{get, post};
use tracing::info;

use crate::runner::Runner;

use crate::server::routes;

/// State shared with all routes.
#[derive(Clone)]
pub struct AppState {
    /// Map of agent name → `Runner`.
    pub runners: Arc<HashMap<String, Arc<Runner>>>,
}

impl std::fmt::Debug for AppState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AppState")
            .field("agents", &self.runners.keys().collect::<Vec<_>>())
            .finish()
    }
}

/// Build the axum router.
pub fn build_router(state: AppState) -> Router {
    Router::new()
        .route("/list-agents", get(routes::list_agents))
        .route("/run", post(routes::run))
        .route("/run_sse", post(routes::run_sse))
        .route(
            "/apps/:app/users/:user/sessions",
            get(routes::list_sessions).post(routes::create_session),
        )
        .route(
            "/apps/:app/users/:user/sessions/:session",
            get(routes::get_session).delete(routes::delete_session),
        )
        .with_state(state)
}

/// Bind and serve.
pub async fn serve(addr: SocketAddr, state: AppState) -> crate::error::Result<()> {
    let app = build_router(state);
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .map_err(|e| crate::error::Error::other(format!("bind {addr}: {e}")))?;
    info!("adk-server listening on http://{addr}");
    axum::serve(listener, app)
        .await
        .map_err(|e| crate::error::Error::other(format!("serve: {e}")))
}
