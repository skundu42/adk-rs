//! End-to-end integration tests for the axum dev server.
//!
//! Boots `build_router` with an in-memory session service and a `MockModel`
//! and drives requests through `tower::ServiceExt::oneshot` — no real socket.

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
    AppState {
        runners: Arc::new(runners),
    }
}

async fn json_body(resp: axum::response::Response) -> Value {
    let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

#[tokio::test]
async fn list_agents_returns_registered_names() {
    let app = build_router(make_state());
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
    let v = json_body(resp).await;
    assert_eq!(v["agents"], json!(["greet"]));
}

#[tokio::test]
async fn run_executes_agent_and_returns_events() {
    let app = build_router(make_state());
    let body = json!({"agent": "greet", "message": "hello"}).to_string();
    let resp = app
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/run")
                .header("content-type", "application/json")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let v = json_body(resp).await;
    let events = v.as_array().expect("response is a JSON array");
    assert!(!events.is_empty(), "expected at least one event");
    // Don't depend on the exact event shape — just confirm our MockModel's
    // canned text reached the wire.
    let dump = serde_json::to_string(&v).unwrap();
    assert!(
        dump.contains("ok-from-mock"),
        "expected mock text in response, got {dump}"
    );
}

#[tokio::test]
async fn run_returns_404_for_unknown_agent() {
    let app = build_router(make_state());
    let body = json!({"agent": "nope", "message": "x"}).to_string();
    let resp = app
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/run")
                .header("content-type", "application/json")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn session_create_list_get_delete_round_trip() {
    let app = build_router(make_state());

    // Create
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/apps/test-app/users/alice/sessions")
                .header("content-type", "application/json")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let session = json_body(resp).await;
    let session_id = session["id"].as_str().unwrap().to_string();

    // List
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/apps/test-app/users/alice/sessions")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let v = json_body(resp).await;
    let ids: Vec<&str> = v
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["id"].as_str().unwrap())
        .collect();
    assert!(ids.contains(&session_id.as_str()));

    // Get
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!("/apps/test-app/users/alice/sessions/{session_id}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // Delete
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
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    // Get again → 404
    let resp = app
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!("/apps/test-app/users/alice/sessions/{session_id}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}
