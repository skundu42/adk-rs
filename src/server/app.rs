//! Server bootstrap.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;

use axum::Router;
use axum::extract::State;
use axum::http::{HeaderValue, Request, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use tracing::{info, warn};

use crate::runner::Runner;

use crate::server::routes;

/// State shared with all routes.
#[derive(Clone)]
pub struct AppState {
    /// Map of agent name → `Runner`.
    pub runners: Arc<HashMap<String, Arc<Runner>>>,
    /// Optional bearer token required on `Authorization: Bearer <token>`.
    /// When `None`, no authentication is enforced by the router (the
    /// transport-level [`serve`] guard still refuses non-loopback binds in
    /// that case).
    pub auth_token: Option<Arc<String>>,
}

impl AppState {
    /// Construct an [`AppState`] with no authentication. Loopback-only by
    /// default — use [`serve_with`] to opt out.
    pub fn unauthenticated(runners: Arc<HashMap<String, Arc<Runner>>>) -> Self {
        Self {
            runners,
            auth_token: None,
        }
    }

    /// Construct an [`AppState`] requiring `Authorization: Bearer <token>`
    /// on every request.
    pub fn with_bearer_token(
        runners: Arc<HashMap<String, Arc<Runner>>>,
        token: impl Into<String>,
    ) -> Self {
        Self {
            runners,
            auth_token: Some(Arc::new(token.into())),
        }
    }
}

impl std::fmt::Debug for AppState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AppState")
            .field("agents", &self.runners.keys().collect::<Vec<_>>())
            .field("auth_token", &self.auth_token.as_ref().map(|_| "<set>"))
            .finish()
    }
}

/// Build the axum router. When `state.auth_token` is `Some`, every route is
/// wrapped in a bearer-token check.
pub fn build_router(state: AppState) -> Router {
    let inner = Router::new()
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
        );
    if state.auth_token.is_some() {
        let token_state = state.clone();
        inner
            .route_layer(middleware::from_fn_with_state(token_state, require_bearer))
            .with_state(state)
    } else {
        inner.with_state(state)
    }
}

/// Bearer-token middleware. Compares the `Authorization: Bearer ...` header
/// against `state.auth_token` in constant time.
async fn require_bearer(
    State(state): State<AppState>,
    req: Request<axum::body::Body>,
    next: Next,
) -> Response {
    let Some(expected) = state.auth_token.as_ref() else {
        return next.run(req).await;
    };
    let presented = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "))
        .or_else(|| {
            req.headers()
                .get(header::AUTHORIZATION)
                .and_then(|v| v.to_str().ok())
                .and_then(|s| s.strip_prefix("bearer "))
        });
    let ok = presented
        .map(|tok| constant_time_eq(expected.as_bytes(), tok.as_bytes()))
        .unwrap_or(false);
    if ok {
        next.run(req).await
    } else {
        let mut resp = (StatusCode::UNAUTHORIZED, "unauthorized").into_response();
        resp.headers_mut().insert(
            header::WWW_AUTHENTICATE,
            HeaderValue::from_static("Bearer realm=\"adk-rs\""),
        );
        resp
    }
}

/// Constant-time byte comparison. Returns false immediately on length
/// mismatch (length itself is not secret here — bearer tokens have a fixed
/// length per deployment).
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

/// Options for [`serve_with`].
#[derive(Debug, Clone, Default)]
pub struct ServeOptions {
    /// If true, allow binding to a non-loopback address even without
    /// authentication. Default: false. Without this flag, [`serve_with`]
    /// refuses to bind a non-loopback address that has no auth token —
    /// otherwise anyone reachable on the network could drive the agents and
    /// read every session's history.
    pub dangerously_allow_unauthenticated_remote: bool,
}

/// Bind and serve. Default behaviour: refuses non-loopback binds unless the
/// state has an auth token or [`ServeOptions::dangerously_allow_unauthenticated_remote`]
/// is set. Always logs a clear warning when serving on a non-loopback
/// interface so accidental exposure can't happen silently.
pub async fn serve(addr: SocketAddr, state: AppState) -> crate::error::Result<()> {
    serve_with(addr, state, ServeOptions::default()).await
}

/// Like [`serve`], but accepts explicit [`ServeOptions`].
pub async fn serve_with(
    addr: SocketAddr,
    state: AppState,
    opts: ServeOptions,
) -> crate::error::Result<()> {
    validate_bind_policy(addr, state.auth_token.is_some(), &opts)?;
    if !addr.ip().is_loopback() {
        let has_auth = state.auth_token.is_some();
        warn!(
            "adk-server bound on non-loopback {addr}: anyone reachable on this network can drive your agents{} — proceed only if this is what you intended",
            if has_auth {
                " (bearer token required)"
            } else {
                " AND NO AUTHENTICATION IS ENFORCED"
            }
        );
    }
    let app = build_router(state);
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .map_err(|e| crate::error::Error::other(format!("bind {addr}: {e}")))?;
    info!("adk-server listening on http://{addr}");
    axum::serve(listener, app)
        .await
        .map_err(|e| crate::error::Error::other(format!("serve: {e}")))
}

fn validate_bind_policy(
    addr: SocketAddr,
    has_auth: bool,
    opts: &ServeOptions,
) -> crate::error::Result<()> {
    if !addr.ip().is_loopback() && !has_auth && !opts.dangerously_allow_unauthenticated_remote {
        return Err(crate::error::Error::config(format!(
            "refusing to bind dev server on non-loopback address {addr} without auth — \
             set an auth token via AppState::with_bearer_token(...) or pass \
             ServeOptions::dangerously_allow_unauthenticated_remote=true to opt out"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{IpAddr, Ipv4Addr};

    fn empty_state(token: Option<&str>) -> AppState {
        let runners: HashMap<String, Arc<Runner>> = HashMap::new();
        match token {
            Some(t) => AppState::with_bearer_token(Arc::new(runners), t),
            None => AppState::unauthenticated(Arc::new(runners)),
        }
    }

    #[tokio::test]
    async fn serve_refuses_non_loopback_without_auth_or_override() {
        let state = empty_state(None);
        let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 0);
        let err = serve(addr, state).await.unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("non-loopback"),
            "expected non-loopback error, got: {msg}"
        );
    }

    #[tokio::test]
    async fn serve_allows_non_loopback_when_auth_set() {
        let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 0);
        validate_bind_policy(addr, true, &ServeOptions::default()).unwrap();
    }

    #[tokio::test]
    async fn bearer_required_when_token_set() {
        use axum::body::{Body, to_bytes};
        use axum::http::{Method, Request};
        use tower::ServiceExt;

        let state = empty_state(Some("topsecret"));
        let app = build_router(state);

        // Missing token → 401.
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/list-agents")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        let www = resp
            .headers()
            .get(header::WWW_AUTHENTICATE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        assert!(www.contains("Bearer"));
        let _ = to_bytes(resp.into_body(), usize::MAX).await.unwrap();

        // Wrong token → 401.
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/list-agents")
                    .header(header::AUTHORIZATION, "Bearer wrong")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

        // Correct token → 200.
        let resp = app
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/list-agents")
                    .header(header::AUTHORIZATION, "Bearer topsecret")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn no_auth_required_when_token_absent() {
        use axum::body::Body;
        use axum::http::{Method, Request};
        use tower::ServiceExt;

        let state = empty_state(None);
        let app = build_router(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/list-agents")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }
}
