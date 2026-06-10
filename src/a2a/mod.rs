//! Agent-to-Agent (A2A) JSON-RPC protocol — client, server, and task
//! persistence.
//!
//! Wire-compatible with the public Google A2A specification:
//!
//! - [`RemoteA2aAgent`] (client) is a [`crate::agents::BaseAgent`] that
//!   proxies calls to a remote A2A server via `message/send` (sync) or
//!   `message/stream` (SSE).
//! - [`server::A2aState`] + [`server::router`] expose any local
//!   [`crate::runner::Runner`] as an A2A endpoint, serving:
//!   - `GET /.well-known/agent.json` ([`types::AgentCard`])
//!   - `POST /` (JSON-RPC) dispatching `message/send`, `message/stream`,
//!     `tasks/get`, `tasks/cancel`, `tasks/resubscribe`
//! - [`task_service::TaskService`] is the persistence trait for
//!   [`types::Task`] state. The default
//!   [`task_service::InMemoryTaskService`] is shipped in this crate; users
//!   can plug in their own Redis / SQL / Firestore backend.
//!
//! Push notifications (`tasks/pushNotificationConfig/set|get|list|delete`)
//! are implemented: registered webhook configs receive task status updates
//! via [`push_notifier::PushNotifier`].

mod client;
pub mod mapping;
pub mod push_notifier;
pub mod server;
pub mod task_service;
pub mod types;

pub use client::{RemoteA2aAgent, RemoteA2aConfig};
pub use push_notifier::PushNotifier;
pub use server::{A2aServerConfig, A2aState, ServeOptions, router, serve, serve_with};
pub use task_service::{InMemoryTaskService, TaskService, TaskUpdate};
pub use types::{
    A2aError, A2aRequest, A2aResponse, AgentAuthentication, AgentCapabilities, AgentCard,
    AgentProvider, AgentSkill, Artifact, ArtifactUpdateKind, FilePayload,
    GetTaskPushNotificationConfigParams, ListTaskPushNotificationConfigResult, Message,
    MessageKind, MessageRole, MessageSendConfiguration, MessageSendParams, Part,
    PushNotificationAuthenticationInfo, PushNotificationConfig, StatusUpdateKind,
    StreamingMessageResult, Task, TaskArtifactUpdateEvent, TaskIdParams, TaskKind,
    TaskPushNotificationConfig, TaskQueryParams, TaskState, TaskStatus, TaskStatusUpdateEvent,
    method,
};
