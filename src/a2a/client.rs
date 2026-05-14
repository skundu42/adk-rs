//! [`RemoteA2aAgent`] — a [`BaseAgent`] that proxies to a remote A2A agent.
//!
//! Speaks the A2A spec on the wire: `message/send` (sync) or
//! `message/stream` (SSE), with full [`Task`] semantics. Optionally fetches
//! the remote [`AgentCard`] at construction so the local agent's
//! description and capabilities mirror the remote.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use async_stream::try_stream;
use async_trait::async_trait;
use eventsource_stream::Eventsource;
use futures::StreamExt;
use reqwest::Client;
use reqwest::header::{ACCEPT, CONTENT_TYPE, HeaderMap, HeaderName, HeaderValue};
use serde_json::Value;
use tracing::debug;
use uuid::Uuid;

use crate::agents::BaseAgent;
use crate::core::{Event, EventStream, InvocationContext, LlmResponse};
use crate::error::{Error, Result};

use super::mapping::{adk_part_to_a2a, message_to_content};
use super::types::{
    A2aRequest, A2aResponse, AgentCard, Message, MessageKind, MessageRole, MessageSendParams,
    Part as A2aPart, StreamingMessageResult, Task, TaskState, method,
};

/// Configuration for [`RemoteA2aAgent`].
#[derive(Debug, Clone)]
pub struct RemoteA2aConfig {
    /// Local name for the proxy agent. If the remote [`AgentCard`] is
    /// fetched at construction, the proxy adopts the card's name instead.
    pub name: String,
    /// Local description (overridden by the agent card when fetched).
    pub description: String,
    /// JSON-RPC endpoint URL of the remote A2A server.
    pub url: String,
    /// Optional URL of the remote agent card. If set, the client fetches it
    /// at construction and copies the `description` / `url` / capability
    /// flags onto this proxy. When `None`, no discovery is performed.
    pub agent_card_url: Option<String>,
    /// Extra HTTP headers (e.g. `Authorization`).
    pub headers: HashMap<String, String>,
    /// Per-request timeout.
    pub timeout: Duration,
    /// If true, use `message/stream` (SSE) instead of `message/send`.
    pub stream: bool,
}

impl Default for RemoteA2aConfig {
    fn default() -> Self {
        Self {
            name: String::new(),
            description: String::new(),
            url: String::new(),
            agent_card_url: None,
            headers: HashMap::new(),
            timeout: Duration::from_secs(120),
            stream: false,
        }
    }
}

/// Remote A2A agent. Implements [`BaseAgent`] by dispatching the user's
/// turn to a remote A2A server and converting the resulting [`Task`]
/// (or streamed update events) into a stream of ADK [`Event`]s.
pub struct RemoteA2aAgent {
    name: String,
    description: String,
    cfg: RemoteA2aConfig,
    http: Client,
    extra_headers: HeaderMap,
}

impl std::fmt::Debug for RemoteA2aAgent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RemoteA2aAgent")
            .field("name", &self.name)
            .field("url", &self.cfg.url)
            .field("stream", &self.cfg.stream)
            .finish()
    }
}

impl RemoteA2aAgent {
    /// Construct without contacting the remote.
    pub fn new(cfg: RemoteA2aConfig) -> Result<Self> {
        Self::build(cfg, None)
    }

    /// Construct and (if `cfg.agent_card_url` is `Some`) fetch the remote
    /// [`AgentCard`] to populate the proxy's name/description.
    pub async fn connect(cfg: RemoteA2aConfig) -> Result<Self> {
        let card = if let Some(card_url) = cfg.agent_card_url.clone() {
            Some(fetch_agent_card(&cfg, &card_url).await?)
        } else {
            None
        };
        Self::build(cfg, card)
    }

    fn build(cfg: RemoteA2aConfig, card: Option<AgentCard>) -> Result<Self> {
        if cfg.name.is_empty() && card.is_none() {
            return Err(Error::config(
                "RemoteA2aConfig.name is empty (and no agent_card_url to discover)",
            ));
        }
        if cfg.url.is_empty() {
            return Err(Error::config("RemoteA2aConfig.url is empty"));
        }
        if cfg
            .headers
            .keys()
            .any(|k| header_looks_credential_bearing(k.as_str()))
        {
            crate::transport_security::require_secure_url(&cfg.url, "RemoteA2aConfig.url")?;
        }
        let mut headers = HeaderMap::new();
        for (k, v) in &cfg.headers {
            let name = HeaderName::from_bytes(k.as_bytes())
                .map_err(|e| Error::config(format!("invalid header {k}: {e}")))?;
            let value = HeaderValue::from_str(v)
                .map_err(|e| Error::config(format!("invalid header value: {e}")))?;
            headers.insert(name, value);
        }
        let http = Client::builder()
            .timeout(cfg.timeout)
            .user_agent(concat!("adk-rs/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|e| Error::other(format!("A2A HTTP client: {e}")))?;
        let (name, description) = match card {
            Some(c) => (c.name, c.description),
            None => (cfg.name.clone(), cfg.description.clone()),
        };
        Ok(Self {
            name,
            description,
            cfg,
            http,
            extra_headers: headers,
        })
    }

    fn build_headers(&self, accept_sse: bool) -> HeaderMap {
        let mut h = self.extra_headers.clone();
        h.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        h.insert(
            ACCEPT,
            if accept_sse {
                HeaderValue::from_static("application/json, text/event-stream")
            } else {
                HeaderValue::from_static("application/json")
            },
        );
        h
    }

    /// Synchronously dispatch [`method::MESSAGE_SEND`] and return the final
    /// [`Task`].
    pub async fn message_send(&self, params: MessageSendParams) -> Result<Task> {
        let req = A2aRequest {
            jsonrpc: "2.0".into(),
            id: Some(Value::String(Uuid::new_v4().to_string())),
            method: method::MESSAGE_SEND.into(),
            params: Some(serde_json::to_value(&params)?),
        };
        let body = serde_json::to_vec(&req)?;
        let resp = self
            .http
            .post(&self.cfg.url)
            .headers(self.build_headers(false))
            .body(body)
            .send()
            .await
            .map_err(|e| Error::other(format!("A2A request: {e}")))?;
        if !resp.status().is_success() {
            return Err(Error::other(format!(
                "A2A HTTP error ({}): {}",
                resp.status(),
                resp.text().await.unwrap_or_default()
            )));
        }
        let env: A2aResponse = resp
            .json()
            .await
            .map_err(|e| Error::other(format!("A2A decode: {e}")))?;
        if let Some(err) = env.error {
            return Err(Error::other(err.to_string()));
        }
        let result = env
            .result
            .ok_or_else(|| Error::other("A2A response missing result"))?;
        let task: Task = serde_json::from_value(result)
            .map_err(|e| Error::other(format!("A2A task decode: {e}")))?;
        Ok(task)
    }

    /// Open an SSE channel for [`method::MESSAGE_STREAM`]. Yields each
    /// [`StreamingMessageResult`] in order.
    pub async fn message_stream(
        &self,
        params: MessageSendParams,
    ) -> Result<std::pin::Pin<Box<dyn futures::Stream<Item = Result<StreamingMessageResult>> + Send>>>
    {
        let req = A2aRequest {
            jsonrpc: "2.0".into(),
            id: Some(Value::String(Uuid::new_v4().to_string())),
            method: method::MESSAGE_STREAM.into(),
            params: Some(serde_json::to_value(&params)?),
        };
        let body = serde_json::to_vec(&req)?;
        let resp = self
            .http
            .post(&self.cfg.url)
            .headers(self.build_headers(true))
            .body(body)
            .send()
            .await
            .map_err(|e| Error::other(format!("A2A streaming request: {e}")))?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(Error::other(format!("A2A HTTP error ({status}): {body}")));
        }
        let ctype = resp
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_ascii_lowercase();
        let bytes = resp.bytes_stream();
        if !ctype.starts_with("text/event-stream") {
            // Some servers reply with a single JSON body even on /stream.
            // We accept that as a single-frame stream.
            let body = bytes
                .map(|r| r.map_err(|e| Error::other(format!("A2A body: {e}"))))
                .collect::<Vec<_>>()
                .await
                .into_iter()
                .collect::<Result<Vec<_>>>()?;
            let bytes: Vec<u8> = body.iter().flat_map(|b| b.to_vec()).collect();
            let env: A2aResponse = serde_json::from_slice(&bytes)
                .map_err(|e| Error::other(format!("A2A decode: {e}")))?;
            return Ok(Box::pin(futures::stream::iter(vec![
                streaming_result_from_envelope(env),
            ])));
        }
        let stream = try_stream! {
            let sse = bytes.eventsource();
            tokio::pin!(sse);
            while let Some(item) = sse.next().await {
                let event = item.map_err(|e| Error::other(format!("A2A SSE: {e}")))?;
                let data = event.data.trim();
                if data.is_empty() { continue; }
                let env: A2aResponse = match serde_json::from_str(data) {
                    Ok(env) => env,
                    Err(e) => {
                        debug!("A2A SSE malformed envelope: {e}; data={data}");
                        continue;
                    }
                };
                if let Some(err) = env.error {
                    Err(Error::other(err.to_string()))?;
                }
                let Some(result) = env.result else { continue };
                let res: StreamingMessageResult = match serde_json::from_value(result.clone()) {
                    Ok(r) => r,
                    Err(e) => {
                        debug!("A2A SSE result decode: {e}; raw={result}");
                        continue;
                    }
                };
                let is_final = matches!(&res,
                    StreamingMessageResult::Status(s) if s.is_final
                ) || matches!(&res,
                    StreamingMessageResult::Task(t) if t.status.state.is_terminal()
                );
                yield res;
                if is_final {
                    break;
                }
            }
        };
        Ok(Box::pin(stream))
    }
}

fn streaming_result_from_envelope(env: A2aResponse) -> Result<StreamingMessageResult> {
    if let Some(err) = env.error {
        return Err(Error::other(err.to_string()));
    }
    let result = env
        .result
        .ok_or_else(|| Error::other("A2A response missing result"))?;
    serde_json::from_value(result).map_err(|e| Error::other(format!("A2A decode: {e}")))
}

async fn fetch_agent_card(cfg: &RemoteA2aConfig, url: &str) -> Result<AgentCard> {
    if cfg
        .headers
        .keys()
        .any(|k| header_looks_credential_bearing(k.as_str()))
    {
        crate::transport_security::require_secure_url(url, "RemoteA2aConfig.agent_card_url")?;
    }
    let mut builder = reqwest::Client::builder().timeout(cfg.timeout);
    builder = builder.user_agent(concat!("adk-rs/", env!("CARGO_PKG_VERSION")));
    let http = builder
        .build()
        .map_err(|e| Error::other(format!("agent-card client: {e}")))?;
    let mut req = http.get(url);
    for (k, v) in &cfg.headers {
        req = req.header(k, v);
    }
    let resp = req
        .send()
        .await
        .map_err(|e| Error::other(format!("agent-card fetch: {e}")))?;
    if !resp.status().is_success() {
        return Err(Error::other(format!(
            "agent-card fetch failed ({}): {}",
            resp.status(),
            resp.text().await.unwrap_or_default()
        )));
    }
    let card: AgentCard = resp
        .json()
        .await
        .map_err(|e| Error::other(format!("agent-card decode: {e}")))?;
    Ok(card)
}

fn header_looks_credential_bearing(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    matches!(
        lower.as_str(),
        "authorization" | "cookie" | "proxy-authorization"
    ) || lower.starts_with("x-api")
        || lower.starts_with("x-auth")
}

/// Build an A2A [`Message`] from an ADK invocation context.
fn message_from_invocation(ctx: &InvocationContext) -> Message {
    let role = MessageRole::User;
    let user_parts: Vec<A2aPart> = ctx
        .user_content
        .as_ref()
        .map(|c| c.parts.iter().map(adk_part_to_a2a).collect())
        .unwrap_or_default();
    let context_id = {
        let s = ctx.session.lock();
        if s.id.is_empty() {
            None
        } else {
            Some(s.id.clone())
        }
    };
    let mut metadata = indexmap::IndexMap::new();
    metadata.insert(
        "user_id".to_string(),
        serde_json::Value::String(ctx.user_id.clone()),
    );
    Message {
        kind: MessageKind::Message,
        role,
        parts: user_parts,
        message_id: Uuid::new_v4().to_string(),
        task_id: None,
        context_id,
        reference_task_ids: Vec::new(),
        metadata: Some(metadata),
    }
}

#[async_trait]
impl BaseAgent for RemoteA2aAgent {
    fn name(&self) -> &str {
        &self.name
    }

    fn description(&self) -> &str {
        &self.description
    }

    async fn run(self: Arc<Self>, ctx: Arc<InvocationContext>) -> Result<EventStream<'static>> {
        let user_msg = message_from_invocation(&ctx);
        let params = MessageSendParams {
            message: user_msg,
            configuration: None,
            metadata: None,
        };
        let proxy_name = self.name.clone();
        if self.cfg.stream {
            let mut stream = self.message_stream(params).await?;
            let stream = try_stream! {
                while let Some(item) = stream.next().await {
                    let res = item?;
                    for ev in stream_result_to_events(&proxy_name, res) {
                        yield ev;
                    }
                }
            };
            return Ok(Box::pin(stream));
        }
        let task = self.message_send(params).await?;
        let events = task_to_events(&proxy_name, &task);
        Ok(Box::pin(futures::stream::iter(events.into_iter().map(Ok))))
    }
}

/// Collapse a final [`Task`] into ADK [`Event`]s, in the same order they
/// appear in `task.history` (skipping the inbound user echo since the
/// runner already recorded it locally).
fn task_to_events(author: &str, task: &Task) -> Vec<Event> {
    let mut out = Vec::new();
    for msg in &task.history {
        if matches!(msg.role, MessageRole::User) {
            continue;
        }
        out.push(message_to_event(author, msg));
    }
    // If the task carried an artifact but no agent message captured the
    // same text, surface the artifact as a final event so the runner sees
    // *something* to attach to the session.
    if out.is_empty() {
        for artifact in &task.artifacts {
            let text: String = artifact
                .parts
                .iter()
                .filter_map(|p| {
                    if let A2aPart::Text { text, .. } = p {
                        Some(text.as_str())
                    } else {
                        None
                    }
                })
                .collect();
            if !text.is_empty() {
                let mut content = crate::genai_types::Content {
                    role: crate::genai_types::Role::Model,
                    parts: vec![crate::genai_types::Part::text(text)],
                };
                content.role = crate::genai_types::Role::Model;
                out.push(Event::new(
                    author,
                    LlmResponse {
                        content: Some(content),
                        ..Default::default()
                    },
                ));
            }
        }
    }
    out
}

fn message_to_event(author: &str, msg: &Message) -> Event {
    let content = message_to_content(msg);
    Event::new(
        author,
        LlmResponse {
            content: Some(content),
            ..Default::default()
        },
    )
}

/// Convert one streaming update into 0..N ADK events. Status updates with
/// a `message` field carry an agent reply we surface immediately; artifact
/// updates surface as model text. The final `Task` snapshot is squelched
/// because everything it contains has already been streamed.
fn stream_result_to_events(author: &str, res: StreamingMessageResult) -> Vec<Event> {
    match res {
        StreamingMessageResult::Status(s) => match s.status.message {
            Some(m) if !matches!(m.role, MessageRole::User) => vec![message_to_event(author, &m)],
            _ => Vec::new(),
        },
        StreamingMessageResult::Artifact(a) => {
            let mut parts: Vec<crate::genai_types::Part> = Vec::new();
            for p in &a.artifact.parts {
                if let A2aPart::Text { text, .. } = p {
                    parts.push(crate::genai_types::Part::text(text.clone()));
                }
            }
            if parts.is_empty() {
                return Vec::new();
            }
            let content = crate::genai_types::Content {
                role: crate::genai_types::Role::Model,
                parts,
            };
            let mut ev = Event::new(
                author,
                LlmResponse {
                    content: Some(content),
                    ..Default::default()
                },
            );
            ev.partial = (a.artifact.last_chunk != Some(true)).then_some(true);
            ev.turn_complete = a.artifact.last_chunk;
            vec![ev]
        }
        StreamingMessageResult::Message(m) => {
            if matches!(m.role, MessageRole::User) {
                Vec::new()
            } else {
                vec![message_to_event(author, &m)]
            }
        }
        StreamingMessageResult::Task(t) => {
            // Only emit if the task carries history we haven't seen yet
            // (terminal snapshot at the end of the stream).
            if t.status.state.is_terminal() && t.status.state == TaskState::Failed {
                if let Some(m) = t.status.message {
                    return vec![message_to_event(author, &m)];
                }
            }
            Vec::new()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::a2a::types::{
        AgentCapabilities, Artifact, StatusUpdateKind, TaskArtifactUpdateEvent, TaskKind,
        TaskStatus, TaskStatusUpdateEvent,
    };
    use serde_json::json;
    use wiremock::matchers::{body_partial_json, method as m, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn config(url: String, stream: bool) -> RemoteA2aConfig {
        RemoteA2aConfig {
            name: "remote".into(),
            description: "remote agent".into(),
            url,
            timeout: Duration::from_secs(5),
            stream,
            ..RemoteA2aConfig::default()
        }
    }

    fn fake_task(text: &str) -> Task {
        Task {
            kind: TaskKind::Task,
            id: "t-1".into(),
            context_id: "ctx-1".into(),
            status: TaskStatus {
                state: TaskState::Completed,
                message: None,
                timestamp: None,
            },
            artifacts: vec![Artifact {
                artifact_id: "a-1".into(),
                name: None,
                description: None,
                parts: vec![A2aPart::text(text)],
                index: None,
                append: None,
                last_chunk: Some(true),
                metadata: None,
            }],
            history: vec![Message {
                kind: MessageKind::Message,
                role: MessageRole::Agent,
                parts: vec![A2aPart::text(text)],
                message_id: "m-1".into(),
                task_id: Some("t-1".into()),
                context_id: Some("ctx-1".into()),
                reference_task_ids: Vec::new(),
                metadata: None,
            }],
            metadata: None,
        }
    }

    #[tokio::test]
    async fn rejects_missing_name_and_url() {
        assert!(RemoteA2aAgent::new(RemoteA2aConfig::default()).is_err());
        let mut c = RemoteA2aConfig::default();
        c.name = "x".into();
        assert!(RemoteA2aAgent::new(c).is_err());
    }

    #[tokio::test]
    async fn rejects_auth_header_over_plaintext_http() {
        let mut headers = HashMap::new();
        headers.insert("Authorization".into(), "Bearer secret".into());
        let err = RemoteA2aAgent::new(RemoteA2aConfig {
            name: "remote".into(),
            url: "http://example.com/".into(),
            headers,
            ..RemoteA2aConfig::default()
        })
        .unwrap_err();
        assert!(err.to_string().to_lowercase().contains("https"));
    }

    #[tokio::test]
    async fn message_send_returns_task() {
        let server = MockServer::start().await;
        Mock::given(m("POST"))
            .and(path("/a2a"))
            .and(body_partial_json(json!({"method": method::MESSAGE_SEND})))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "jsonrpc":"2.0","id":"1",
                "result": fake_task("hello from remote")
            })))
            .mount(&server)
            .await;
        let agent = RemoteA2aAgent::new(config(format!("{}/a2a", server.uri()), false)).unwrap();
        let task = agent
            .message_send(MessageSendParams {
                message: Message::user_text("hi"),
                configuration: None,
                metadata: None,
            })
            .await
            .unwrap();
        assert_eq!(task.status.state, TaskState::Completed);
        assert_eq!(task.artifacts.len(), 1);
    }

    #[tokio::test]
    async fn message_stream_yields_status_then_artifact_then_terminal_task() {
        let server = MockServer::start().await;
        let status = TaskStatusUpdateEvent {
            kind: StatusUpdateKind::StatusUpdate,
            task_id: "t-1".into(),
            context_id: "ctx-1".into(),
            status: TaskStatus {
                state: TaskState::Working,
                message: None,
                timestamp: None,
            },
            is_final: false,
            metadata: None,
        };
        let artifact = TaskArtifactUpdateEvent {
            kind: super::super::types::ArtifactUpdateKind::ArtifactUpdate,
            task_id: "t-1".into(),
            context_id: "ctx-1".into(),
            artifact: Artifact {
                artifact_id: "a-1".into(),
                name: None,
                description: None,
                parts: vec![A2aPart::text("hello")],
                index: None,
                append: Some(false),
                last_chunk: Some(true),
                metadata: None,
            },
            metadata: None,
        };
        let final_status = TaskStatusUpdateEvent {
            kind: StatusUpdateKind::StatusUpdate,
            task_id: "t-1".into(),
            context_id: "ctx-1".into(),
            status: TaskStatus {
                state: TaskState::Completed,
                message: None,
                timestamp: None,
            },
            is_final: true,
            metadata: None,
        };
        let sse = format!(
            "data: {}\n\ndata: {}\n\ndata: {}\n\n",
            json!({"jsonrpc":"2.0","id":"1","result": status}),
            json!({"jsonrpc":"2.0","id":"1","result": artifact}),
            json!({"jsonrpc":"2.0","id":"1","result": final_status}),
        );
        Mock::given(m("POST"))
            .and(path("/a2a"))
            .respond_with(
                ResponseTemplate::new(200).set_body_raw(sse.into_bytes(), "text/event-stream"),
            )
            .mount(&server)
            .await;
        let agent = RemoteA2aAgent::new(config(format!("{}/a2a", server.uri()), true)).unwrap();
        let mut stream = agent
            .message_stream(MessageSendParams {
                message: Message::user_text("hi"),
                configuration: None,
                metadata: None,
            })
            .await
            .unwrap();
        let mut got = Vec::new();
        while let Some(r) = stream.next().await {
            got.push(r.unwrap());
        }
        assert!(matches!(got[0], StreamingMessageResult::Status(_)));
        assert!(matches!(got[1], StreamingMessageResult::Artifact(_)));
        assert!(matches!(got[2], StreamingMessageResult::Status(_)));
    }

    #[tokio::test]
    async fn connect_fetches_agent_card_and_adopts_name() {
        let server = MockServer::start().await;
        let card = AgentCard {
            name: "remote-name".into(),
            description: "from card".into(),
            url: format!("{}/a2a", server.uri()),
            provider: None,
            version: "1.0".into(),
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
        };
        Mock::given(m("GET"))
            .and(path("/.well-known/agent.json"))
            .respond_with(ResponseTemplate::new(200).set_body_json(card))
            .mount(&server)
            .await;
        let cfg = RemoteA2aConfig {
            name: "fallback".into(),
            description: "fallback".into(),
            url: format!("{}/a2a", server.uri()),
            agent_card_url: Some(format!("{}/.well-known/agent.json", server.uri())),
            timeout: Duration::from_secs(5),
            ..RemoteA2aConfig::default()
        };
        let agent = RemoteA2aAgent::connect(cfg).await.unwrap();
        assert_eq!(agent.name(), "remote-name");
        assert_eq!(agent.description(), "from card");
    }
}
