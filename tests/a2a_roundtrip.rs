//! End-to-end A2A roundtrip: a real axum server hosts a [`Runner`], and a
//! [`RemoteA2aAgent`] talks to it over the Google A2A protocol.

#![cfg(all(feature = "a2a", feature = "testing"))]

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use adk_rs::a2a::{
    A2aServerConfig, A2aState, AgentCapabilities, AgentCard, RemoteA2aAgent, RemoteA2aConfig,
    router,
};
use adk_rs::agents::{BaseAgent, LlmAgent};
use adk_rs::core::Model;
use adk_rs::core::testing::MockModel;
use adk_rs::core::{InvocationContext, InvocationOrigin, RunConfig, Session, SessionService};
use adk_rs::genai_types::Content;
use adk_rs::runner::Runner;
use adk_rs::services::mem::InMemorySessionService;

use futures::StreamExt;
use parking_lot::Mutex;

fn agent_card(base_url: &str) -> AgentCard {
    AgentCard {
        name: "remote_agent".into(),
        description: "loopback greeter".into(),
        url: format!("{base_url}/"),
        provider: None,
        version: "0.1.0".into(),
        documentation_url: None,
        capabilities: AgentCapabilities {
            streaming: true,
            push_notifications: false,
            state_transition_history: false,
        },
        authentication: None,
        default_input_modes: vec!["text/plain".into()],
        default_output_modes: vec!["text/plain".into()],
        skills: vec![],
    }
}

async fn spawn_remote() -> (std::net::SocketAddr, tokio::task::JoinHandle<()>) {
    let model = Arc::new(MockModel::new("remote-mock"));
    model.push_text("remote says hi");
    let agent: Arc<dyn BaseAgent> = Arc::new(
        LlmAgent::builder("remote_agent")
            .model(model as Arc<dyn Model>)
            .instruction("be terse")
            .build()
            .unwrap(),
    );
    let runner = Arc::new(
        Runner::builder()
            .app_name("a2a-loopback")
            .agent(agent)
            .session_service(Arc::new(InMemorySessionService::new()))
            .auto_create_session(true)
            .build()
            .unwrap(),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let base = format!("http://{addr}");
    let state = A2aState::new(runner, A2aServerConfig::new(agent_card(&base)));
    let app = router(state);
    let handle = tokio::spawn(async move {
        axum::serve(listener, app).await.ok();
    });
    (addr, handle)
}

fn ctx_for(user_text: &str) -> Arc<InvocationContext> {
    Arc::new(InvocationContext {
        app_name: "client-app".into(),
        user_id: "u".into(),
        invocation_id: InvocationContext::new_id(),
        session: Arc::new(Mutex::new(Session::new("client-app", "u", "s"))),
        session_service: Arc::new(InMemorySessionService::new()) as Arc<dyn SessionService>,
        artifact_service: None,
        memory_service: None,
        credential_service: None,
        run_config: RunConfig::default(),
        origin: InvocationOrigin::Api,
        user_content: Some(Content::user_text(user_text)),
        llm_call_count: Arc::new(Mutex::new(0)),
        cancellation: Default::default(),
        attributes: Arc::new(Mutex::new(HashMap::new())),
    })
}

#[tokio::test]
async fn remote_a2a_agent_proxies_message_send() {
    let (addr, _handle) = spawn_remote().await;
    let agent = Arc::new(
        RemoteA2aAgent::new(RemoteA2aConfig {
            name: "remote".into(),
            description: "loopback".into(),
            url: format!("http://{addr}/"),
            timeout: Duration::from_secs(5),
            ..RemoteA2aConfig::default()
        })
        .unwrap(),
    );
    let mut stream = agent.run(ctx_for("ping")).await.unwrap();
    let mut collected = Vec::new();
    while let Some(ev) = stream.next().await {
        collected.push(ev.unwrap());
    }
    assert!(collected.iter().any(|e| {
        e.response
            .content
            .as_ref()
            .map(|c| c.text_concat().contains("remote says hi"))
            .unwrap_or(false)
    }));
}

#[tokio::test]
async fn remote_a2a_agent_proxies_message_stream() {
    let (addr, _handle) = spawn_remote().await;
    let agent = Arc::new(
        RemoteA2aAgent::new(RemoteA2aConfig {
            name: "remote".into(),
            description: "loopback".into(),
            url: format!("http://{addr}/"),
            timeout: Duration::from_secs(5),
            stream: true,
            ..RemoteA2aConfig::default()
        })
        .unwrap(),
    );
    let mut stream = agent.run(ctx_for("ping")).await.unwrap();
    let mut collected = Vec::new();
    while let Some(ev) = stream.next().await {
        collected.push(ev.unwrap());
    }
    assert!(collected.iter().any(|e| {
        e.response
            .content
            .as_ref()
            .map(|c| c.text_concat().contains("remote says hi"))
            .unwrap_or(false)
    }));
}

#[tokio::test]
async fn push_notification_round_trip_via_jsonrpc() {
    use adk_rs::a2a::{
        A2aRequest, A2aResponse, PushNotificationConfig, TaskPushNotificationConfig, method,
    };
    use serde_json::{Value, json};
    use wiremock::matchers::{method as m, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    // Start a webhook receiver.
    let webhook = MockServer::start().await;
    Mock::given(m("POST"))
        .and(path("/hook"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&webhook)
        .await;

    // Start the A2A server.
    let (addr, _handle) = spawn_remote().await;

    // Drive message/send to create a task we can attach a config to.
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap();
    let send_body = A2aRequest {
        jsonrpc: "2.0".into(),
        id: Some(Value::String("send-1".into())),
        method: method::MESSAGE_SEND.into(),
        params: Some(json!({
            "message": {
                "kind": "message",
                "role": "user",
                "messageId": "m-1",
                "parts": [{"kind": "text", "text": "hi"}]
            }
        })),
    };
    let resp: A2aResponse = client
        .post(format!("http://{addr}/"))
        .json(&send_body)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let task: adk_rs::a2a::Task = serde_json::from_value(resp.result.unwrap()).unwrap();
    let task_id = task.id;

    // Now register a push config against that task.
    let set_body = A2aRequest {
        jsonrpc: "2.0".into(),
        id: Some(Value::String("set-1".into())),
        method: method::TASKS_PUSH_NOTIFICATION_CONFIG_SET.into(),
        params: Some(
            serde_json::to_value(&TaskPushNotificationConfig {
                task_id: task_id.clone(),
                push_notification_config: PushNotificationConfig {
                    id: None,
                    url: format!("{}/hook", webhook.uri()),
                    token: None,
                    authentication: None,
                },
            })
            .unwrap(),
        ),
    };
    let resp: A2aResponse = client
        .post(format!("http://{addr}/"))
        .json(&set_body)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(resp.error.is_none(), "set failed: {:?}", resp.error);

    // List configs over RPC and check the count.
    let list_body = A2aRequest {
        jsonrpc: "2.0".into(),
        id: Some(Value::String("list-1".into())),
        method: method::TASKS_PUSH_NOTIFICATION_CONFIG_LIST.into(),
        params: Some(json!({"id": task_id})),
    };
    let resp: A2aResponse = client
        .post(format!("http://{addr}/"))
        .json(&list_body)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let listed: Vec<TaskPushNotificationConfig> =
        serde_json::from_value(resp.result.unwrap()).unwrap();
    assert_eq!(listed.len(), 1);

    // Delete the registered config over RPC.
    let delete_body = A2aRequest {
        jsonrpc: "2.0".into(),
        id: Some(Value::String("del-1".into())),
        method: method::TASKS_PUSH_NOTIFICATION_CONFIG_DELETE.into(),
        params: Some(json!({"id": task_id})),
    };
    let resp: A2aResponse = client
        .post(format!("http://{addr}/"))
        .json(&delete_body)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(resp.result.unwrap()["removed"], 1);

    // After deletion, list returns empty.
    let resp: A2aResponse = client
        .post(format!("http://{addr}/"))
        .json(&list_body)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let listed_after: Vec<TaskPushNotificationConfig> =
        serde_json::from_value(resp.result.unwrap()).unwrap();
    assert!(listed_after.is_empty());

    // Webhook actually firing during a task's lifecycle is covered in the
    // unit-level `end_to_end_webhook_fires_on_status_update` test (which can
    // drive `update_status` directly between registration and completion).
    // From the integration layer, all we can observe is the HTTP-level
    // round-trip of `set`/`list`/`delete`.
}

#[tokio::test]
async fn remote_a2a_agent_picks_up_agent_card_via_connect() {
    let (addr, _handle) = spawn_remote().await;
    let agent = RemoteA2aAgent::connect(RemoteA2aConfig {
        name: "fallback".into(),
        description: "fallback".into(),
        url: format!("http://{addr}/"),
        agent_card_url: Some(format!("http://{addr}/.well-known/agent.json")),
        timeout: Duration::from_secs(5),
        ..RemoteA2aConfig::default()
    })
    .await
    .unwrap();
    assert_eq!(agent.name(), "remote_agent");
    assert_eq!(agent.description(), "loopback greeter");
}
