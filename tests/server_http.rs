//! End-to-end integration tests for the axum dev server.
//!
//! Boots `build_router` with an in-memory session service and a `MockModel`
//! and drives requests through `tower::ServiceExt::oneshot` — no real socket.
//! Asserts the Python `adk api_server` wire contract (camelCase JSON,
//! `{"detail": ...}` errors, bare-array responses) that the adk-web dev UI
//! depends on.

#![cfg(all(feature = "server", feature = "testing"))]

use std::collections::HashMap;
use std::sync::Arc;

use adk_rs::agents::{BaseAgent, LlmAgent};
use adk_rs::core::Model;
use adk_rs::core::testing::MockModel;
use adk_rs::runner::Runner;
use adk_rs::server::{AppState, build_router};
use adk_rs::services::mem::InMemorySessionService;

use axum::body::{Body, to_bytes};
use axum::http::{Method, Request, StatusCode};
use serde_json::{Value, json};
use tower::ServiceExt;

fn make_state() -> AppState {
    let model = Arc::new(MockModel::new("mock-server"));
    model.push_text("ok-from-mock");
    let agent: Arc<dyn BaseAgent> = Arc::new(
        LlmAgent::builder("greet")
            .model(model as Arc<dyn Model>)
            .instruction("be terse")
            .build()
            .unwrap(),
    );
    let runner = Runner::builder()
        .app_name("test-app")
        .agent(agent)
        .session_service(Arc::new(InMemorySessionService::new()))
        .auto_create_session(true)
        .build()
        .unwrap();
    let mut runners: HashMap<String, Arc<Runner>> = HashMap::new();
    runners.insert("greet".into(), Arc::new(runner));
    AppState::unauthenticated(Arc::new(runners))
}

async fn json_body(resp: axum::response::Response) -> Value {
    let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

fn get(uri: &str) -> Request<Body> {
    Request::builder()
        .method(Method::GET)
        .uri(uri)
        .body(Body::empty())
        .unwrap()
}

fn post_json(uri: &str, body: Value) -> Request<Body> {
    Request::builder()
        .method(Method::POST)
        .uri(uri)
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap()
}

#[tokio::test]
async fn health_version_and_list_apps() {
    let app = build_router(make_state());

    let resp = app.clone().oneshot(get("/health")).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(json_body(resp).await["status"], "ok");

    let resp = app.clone().oneshot(get("/version")).await.unwrap();
    let v = json_body(resp).await;
    assert_eq!(v["language"], "rust");
    assert!(v["version"].is_string());

    // The UI calls /list-apps with a legacy query param; it's ignored.
    let resp = app
        .oneshot(get("/list-apps?relative_path=./"))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(json_body(resp).await, json!(["test-app"]));
}

#[tokio::test]
async fn run_executes_agent_and_returns_camel_case_events() {
    let app = build_router(make_state());

    // Create the session first (no auto-create assumption on /run).
    let resp = app
        .clone()
        .oneshot(post_json(
            "/apps/test-app/users/alice/sessions",
            json!({"sessionId": "s-1"}),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let body = json!({
        "appName": "test-app",
        "userId": "alice",
        "sessionId": "s-1",
        "newMessage": {"role": "user", "parts": [{"text": "hello"}]},
        "streaming": false
    });
    let resp = app.clone().oneshot(post_json("/run", body)).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let v = json_body(resp).await;
    let events = v.as_array().expect("response is a bare JSON array");
    assert!(!events.is_empty());
    let final_event = events.last().unwrap();
    assert_eq!(final_event["author"], "greet");
    assert_eq!(final_event["content"]["parts"][0]["text"], "ok-from-mock");
    assert_eq!(final_event["content"]["role"], "model");
    // camelCase + always-present actions maps.
    assert!(final_event["invocationId"].is_string());
    assert!(final_event["timestamp"].is_f64());
    assert!(final_event["actions"]["stateDelta"].is_object());
    assert!(final_event["actions"]["requestedToolConfirmations"].is_object());

    // The session now holds the turn (user + model events).
    let resp = app
        .oneshot(get("/apps/test-app/users/alice/sessions/s-1"))
        .await
        .unwrap();
    let session = json_body(resp).await;
    assert_eq!(session["appName"], "test-app");
    assert_eq!(session["userId"], "alice");
    assert!(session["events"].as_array().unwrap().len() >= 2);
    assert_eq!(session["events"][0]["author"], "user");
}

#[tokio::test]
async fn run_with_snake_case_aliases_also_accepted() {
    let app = build_router(make_state());
    app.clone()
        .oneshot(post_json(
            "/apps/test-app/users/bob/sessions",
            json!({"session_id": "s-2"}),
        ))
        .await
        .unwrap();
    let body = json!({
        "app_name": "test-app",
        "user_id": "bob",
        "session_id": "s-2",
        "new_message": {"role": "user", "parts": [{"text": "hello"}]}
    });
    let resp = app.oneshot(post_json("/run", body)).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn run_returns_404_detail_for_missing_session() {
    let app = build_router(make_state());
    let body = json!({
        "appName": "test-app",
        "userId": "alice",
        "sessionId": "nope",
        "newMessage": {"role": "user", "parts": [{"text": "x"}]}
    });
    let resp = app.oneshot(post_json("/run", body)).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    let v = json_body(resp).await;
    assert!(
        v["detail"].as_str().unwrap().contains("Session not found"),
        "got: {v}"
    );
}

#[tokio::test]
async fn session_crud_round_trip() {
    let app = build_router(make_state());

    // Create with a server-assigned id; the UI posts `null`.
    let resp = app
        .clone()
        .oneshot(post_json("/apps/test-app/users/alice/sessions", Value::Null))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let session = json_body(resp).await;
    let session_id = session["id"].as_str().unwrap().to_string();
    assert_eq!(session["appName"], "test-app");
    assert!(session["lastUpdateTime"].is_f64());

    // Duplicate create with explicit id → 409.
    let resp = app
        .clone()
        .oneshot(post_json(
            "/apps/test-app/users/alice/sessions",
            json!({"sessionId": session_id}),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CONFLICT);
    assert!(
        json_body(resp)
            .await["detail"]
            .as_str()
            .unwrap()
            .contains("already exists")
    );

    // List → bare array containing the session.
    let resp = app
        .clone()
        .oneshot(get("/apps/test-app/users/alice/sessions"))
        .await
        .unwrap();
    let v = json_body(resp).await;
    let ids: Vec<&str> = v
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["id"].as_str().unwrap())
        .collect();
    assert!(ids.contains(&session_id.as_str()));

    // PATCH state delta.
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::PATCH)
                .uri(format!("/apps/test-app/users/alice/sessions/{session_id}"))
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({"stateDelta": {"theme": "dark"}}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(json_body(resp).await["state"]["theme"], "dark");

    // Delete → 200 null.
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::DELETE)
                .uri(format!("/apps/test-app/users/alice/sessions/{session_id}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert!(json_body(resp).await.is_null());

    // Get after delete → 404 {"detail": "Session not found"}.
    let resp = app
        .oneshot(get(&format!(
            "/apps/test-app/users/alice/sessions/{session_id}"
        )))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    assert_eq!(json_body(resp).await["detail"], "Session not found");
}

#[tokio::test]
async fn run_sse_emits_data_frames() {
    let app = build_router(make_state());
    app.clone()
        .oneshot(post_json(
            "/apps/test-app/users/alice/sessions",
            json!({"sessionId": "sse-1"}),
        ))
        .await
        .unwrap();
    let body = json!({
        "appName": "test-app",
        "userId": "alice",
        "sessionId": "sse-1",
        "newMessage": {"role": "user", "parts": [{"text": "hello"}]},
        "streaming": false
    });
    let resp = app.oneshot(post_json("/run_sse", body)).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert!(
        resp.headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .starts_with("text/event-stream")
    );
    let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let text = String::from_utf8_lossy(&bytes);
    // Each frame is `data: <one-line JSON>`; the payloads parse and carry
    // camelCase events.
    let payloads: Vec<Value> = text
        .lines()
        .filter_map(|l| l.strip_prefix("data: "))
        .map(|l| serde_json::from_str(l).expect("each data line is complete JSON"))
        .collect();
    assert!(!payloads.is_empty());
    assert!(
        payloads
            .iter()
            .any(|p| p["content"]["parts"][0]["text"] == "ok-from-mock"),
        "expected mock text in SSE payloads: {text}"
    );
    assert!(payloads.iter().all(|p| p.get("invocationId").is_some()));
}

#[tokio::test]
async fn trace_and_eval_stubs_degrade_gracefully() {
    let app = build_router(make_state());

    let resp = app
        .clone()
        .oneshot(get("/debug/trace/session/whatever"))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(json_body(resp).await, json!([]));

    let resp = app
        .clone()
        .oneshot(get("/debug/trace/some-event"))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);

    let resp = app
        .oneshot(get("/apps/test-app/eval_sets"))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(json_body(resp).await, json!([]));
}

#[tokio::test]
async fn list_agents_legacy_route_still_works() {
    let app = build_router(make_state());
    let resp = app.oneshot(get("/list-agents")).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(json_body(resp).await["agents"], json!(["greet"]));
}

#[tokio::test]
async fn cors_preflight_allowed_for_configured_origin() {
    let state = make_state().with_allow_origins(["http://localhost:4200".to_string()]);
    let app = build_router(state);
    let resp = app
        .oneshot(
            Request::builder()
                .method(Method::OPTIONS)
                .uri("/run")
                .header("origin", "http://localhost:4200")
                .header("access-control-request-method", "POST")
                .header("access-control-request-headers", "content-type")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        resp.headers()
            .get("access-control-allow-origin")
            .and_then(|v| v.to_str().ok()),
        Some("http://localhost:4200")
    );
}
