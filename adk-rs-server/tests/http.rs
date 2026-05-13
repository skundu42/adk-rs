//! HTTP integration test for the adk-server.
#![allow(clippy::unwrap_used)]

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;

use adk_rs_agents::LlmAgent;
use adk_rs_core::testing::MockModel;
use adk_rs_core::{Model, SessionService};
use adk_rs_runner::Runner;
use adk_rs_server::{AppState, serve};
use adk_rs_services_mem::InMemorySessionService;

#[tokio::test]
async fn run_endpoint_returns_events() {
    let m = Arc::new(MockModel::new("mock"));
    m.push_text("hello from agent");
    let agent = Arc::new(
        LlmAgent::builder("greeter")
            .model(m.clone() as Arc<dyn Model>)
            .instruction("greet")
            .build()
            .unwrap(),
    );
    let svc: Arc<dyn SessionService> = Arc::new(InMemorySessionService::new());
    let runner = Arc::new(
        Runner::builder()
            .app_name("hello")
            .agent(agent)
            .session_service(svc)
            .build()
            .unwrap(),
    );
    let mut runners = HashMap::new();
    runners.insert("greeter".to_string(), runner);
    let state = AppState {
        runners: Arc::new(runners),
    };

    // Bind to a random port.
    let addr: SocketAddr = "127.0.0.1:0".parse().unwrap();
    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    let bound = listener.local_addr().unwrap();
    let app = adk_rs_server::build_router(state);
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    // tiny sleep to let listener be ready (not strictly necessary, but safe).
    tokio::task::yield_now().await;

    let url = format!("http://{bound}/run");
    let r = reqwest::Client::new()
        .post(url)
        .json(&serde_json::json!({
            "agent": "greeter",
            "user_id": "u",
            "message": "hi",
        }))
        .send()
        .await
        .unwrap();
    assert!(r.status().is_success(), "status was {}", r.status());
    let events: serde_json::Value = r.json().await.unwrap();
    let arr = events.as_array().unwrap();
    assert!(!arr.is_empty());
    let last = arr.last().unwrap();
    let text = last["content"]["parts"][0]["text"].as_str().unwrap_or("");
    assert!(text.contains("hello from agent"));
    let _ = serve; // keep `serve` referenced (the test uses `build_router`).
}

#[tokio::test]
async fn list_agents_works() {
    let state = AppState {
        runners: Arc::new(HashMap::new()),
    };
    let app = adk_rs_server::build_router(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let bound = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    tokio::task::yield_now().await;
    let r = reqwest::get(format!("http://{bound}/list-agents"))
        .await
        .unwrap();
    assert!(r.status().is_success());
    let v: serde_json::Value = r.json().await.unwrap();
    assert!(v["agents"].is_array());
}
