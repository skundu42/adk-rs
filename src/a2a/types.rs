//! Wire-level types for the Google A2A (Agent-to-Agent) JSON-RPC protocol.
//!
//! Mirrors the public A2A spec closely enough that this crate's clients can
//! talk to Python `google-adk` servers (and vice versa) byte-for-byte. The
//! shapes use spec-style `kind` discriminators on every variant; ADK-native
//! [`Content`](crate::genai_types::Content) and
//! [`Event`](crate::core::Event) live in
//! [`crate::a2a::mapping`].
//!
//! Reference: <https://google.github.io/A2A/specification>.

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// JSON-RPC method names defined by the A2A spec.
pub mod method {
    /// `message/send` — synchronous send, returns a [`super::Task`] (or a
    /// [`super::Message`] if the agent declined to start a task).
    pub const MESSAGE_SEND: &str = "message/send";
    /// `message/stream` — same input as [`MESSAGE_SEND`] but the response is
    /// an SSE channel emitting [`super::TaskStatusUpdateEvent`],
    /// [`super::TaskArtifactUpdateEvent`], and finally [`super::Task`]
    /// / [`super::Message`] payloads.
    pub const MESSAGE_STREAM: &str = "message/stream";
    /// `tasks/get` — look up a [`super::Task`] by id.
    pub const TASKS_GET: &str = "tasks/get";
    /// `tasks/cancel` — request cancellation; server flips the task to
    /// `canceled` and emits a terminal status update.
    pub const TASKS_CANCEL: &str = "tasks/cancel";
    /// `tasks/resubscribe` — re-attach to a task's SSE channel.
    pub const TASKS_RESUBSCRIBE: &str = "tasks/resubscribe";
    /// `tasks/pushNotificationConfig/set` — register a webhook callback for
    /// out-of-band updates on a task.
    pub const TASKS_PUSH_NOTIFICATION_CONFIG_SET: &str = "tasks/pushNotificationConfig/set";
    /// `tasks/pushNotificationConfig/get` — retrieve a registered config.
    pub const TASKS_PUSH_NOTIFICATION_CONFIG_GET: &str = "tasks/pushNotificationConfig/get";
    /// `tasks/pushNotificationConfig/list` — enumerate registered configs
    /// for a task.
    pub const TASKS_PUSH_NOTIFICATION_CONFIG_LIST: &str = "tasks/pushNotificationConfig/list";
    /// `tasks/pushNotificationConfig/delete` — drop a registered config.
    pub const TASKS_PUSH_NOTIFICATION_CONFIG_DELETE: &str = "tasks/pushNotificationConfig/delete";
}

/// JSON-RPC 2.0 request envelope.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct A2aRequest {
    /// Protocol literal `"2.0"`.
    pub jsonrpc: String,
    /// Request id (string or integer, opaque to the server).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<Value>,
    /// Method name (one of [`method`]).
    pub method: String,
    /// Method-specific parameters.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
}

/// JSON-RPC 2.0 response envelope.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct A2aResponse {
    /// Protocol literal `"2.0"`.
    pub jsonrpc: String,
    /// Matches the request id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<Value>,
    /// Result payload (mutually exclusive with `error`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    /// Error payload (mutually exclusive with `result`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<A2aError>,
}

impl A2aResponse {
    /// Success result with the given id.
    pub fn ok(id: Option<Value>, result: Value) -> Self {
        Self {
            jsonrpc: "2.0".into(),
            id,
            result: Some(result),
            error: None,
        }
    }

    /// Error result with the given id.
    pub fn err(id: Option<Value>, error: A2aError) -> Self {
        Self {
            jsonrpc: "2.0".into(),
            id,
            result: None,
            error: Some(error),
        }
    }
}

/// JSON-RPC 2.0 error object. The codes mirror the spec's reserved
/// `-32xxx` range plus a few A2A-specific positive codes documented inline.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct A2aError {
    /// Error code.
    pub code: i32,
    /// Human-readable error message.
    pub message: String,
    /// Optional structured data.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

impl A2aError {
    /// `-32700` — payload was not valid JSON.
    pub const PARSE_ERROR: i32 = -32700;
    /// `-32600` — envelope was malformed (e.g. missing method).
    pub const INVALID_REQUEST: i32 = -32600;
    /// `-32601` — method is unknown.
    pub const METHOD_NOT_FOUND: i32 = -32601;
    /// `-32602` — `params` were invalid for the method.
    pub const INVALID_PARAMS: i32 = -32602;
    /// `-32603` — generic server failure.
    pub const INTERNAL_ERROR: i32 = -32603;
    /// `-32001` — A2A: requested task id is unknown to this server.
    pub const TASK_NOT_FOUND: i32 = -32001;
    /// `-32002` — A2A: task is in a terminal state and can't be canceled.
    pub const TASK_NOT_CANCELABLE: i32 = -32002;

    /// Build with code + message.
    pub fn new(code: i32, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            data: None,
        }
    }
}

impl std::fmt::Display for A2aError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "A2A error {}: {}", self.code, self.message)
    }
}

impl std::error::Error for A2aError {}

// ── Message & Part ──────────────────────────────────────────────────────────

/// A single message exchanged between user and agent. The spec splits these
/// out so the `role` controls how they're rendered and ordered in
/// [`Task::history`].
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Message {
    /// Always `"message"` (the spec uses `kind` to discriminate Task vs
    /// Message vs update events in streaming payloads).
    pub kind: MessageKind,
    /// `"user"` or `"agent"`.
    pub role: MessageRole,
    /// Ordered list of parts that make up the message body.
    pub parts: Vec<Part>,
    /// Caller-supplied id for this message (server echoes it back in
    /// `history`).
    #[serde(rename = "messageId")]
    pub message_id: String,
    /// If this message is tied to an existing task, its id.
    #[serde(default, rename = "taskId", skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
    /// Conversation/context id — multiple tasks may share one.
    #[serde(default, rename = "contextId", skip_serializing_if = "Option::is_none")]
    pub context_id: Option<String>,
    /// Other task ids the caller is asking the agent to take into account.
    #[serde(
        default,
        rename = "referenceTaskIds",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub reference_task_ids: Vec<String>,
    /// Free-form metadata.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<IndexMap<String, Value>>,
}

impl Message {
    /// Convenience: a user message carrying a single text part.
    pub fn user_text(text: impl Into<String>) -> Self {
        Self {
            kind: MessageKind::Message,
            role: MessageRole::User,
            parts: vec![Part::Text {
                text: text.into(),
                metadata: None,
            }],
            message_id: uuid::Uuid::new_v4().to_string(),
            task_id: None,
            context_id: None,
            reference_task_ids: Vec::new(),
            metadata: None,
        }
    }

    /// Convenience: an agent message carrying a single text part.
    pub fn agent_text(text: impl Into<String>) -> Self {
        Self {
            kind: MessageKind::Message,
            role: MessageRole::Agent,
            parts: vec![Part::Text {
                text: text.into(),
                metadata: None,
            }],
            message_id: uuid::Uuid::new_v4().to_string(),
            task_id: None,
            context_id: None,
            reference_task_ids: Vec::new(),
            metadata: None,
        }
    }

    /// Concatenate every [`Part::Text`] in `self.parts` into a single
    /// `String`. Non-text parts are skipped.
    #[must_use]
    pub fn text_concat(&self) -> String {
        let mut out = String::new();
        for p in &self.parts {
            if let Part::Text { text, .. } = p {
                out.push_str(text);
            }
        }
        out
    }
}

/// Discriminator for [`Message`]. Always `"message"` — exists so the
/// streaming SSE payloads (`Task`, `Message`, `*UpdateEvent`) can share a
/// single union shape on the wire.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum MessageKind {
    /// The only valid variant.
    #[default]
    Message,
}

/// `Message.role` — `"user"` for caller-originated content,
/// `"agent"` for everything emitted by the server.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum MessageRole {
    /// Originated from the caller.
    User,
    /// Originated from the server-side agent.
    Agent,
}

/// One content unit inside [`Message::parts`] or [`Artifact::parts`].
///
/// Each variant carries the `kind` discriminator on the wire (e.g.
/// `{"kind":"text","text":"..."}`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Part {
    /// Plain text.
    Text {
        /// The text content.
        text: String,
        /// Optional per-part metadata.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        metadata: Option<IndexMap<String, Value>>,
    },
    /// File reference (either inline bytes or a URI).
    File {
        /// File payload.
        file: FilePayload,
        /// Optional per-part metadata.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        metadata: Option<IndexMap<String, Value>>,
    },
    /// Structured JSON data the caller and agent agreed on.
    Data {
        /// Free-form JSON object.
        data: Value,
        /// Optional per-part metadata.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        metadata: Option<IndexMap<String, Value>>,
    },
}

impl Part {
    /// Convenience: build a text part with no metadata.
    pub fn text(t: impl Into<String>) -> Self {
        Self::Text {
            text: t.into(),
            metadata: None,
        }
    }
}

/// `Part::File.file` payload — base64-encoded bytes or a URI, never both.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum FilePayload {
    /// File embedded directly as base64 bytes.
    Inline {
        /// Display name.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        /// MIME type.
        #[serde(default, rename = "mimeType", skip_serializing_if = "Option::is_none")]
        mime_type: Option<String>,
        /// Base64-encoded bytes.
        bytes: String,
    },
    /// File referenced by URI.
    Uri {
        /// Display name.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        /// MIME type.
        #[serde(default, rename = "mimeType", skip_serializing_if = "Option::is_none")]
        mime_type: Option<String>,
        /// URI of the file.
        uri: String,
    },
}

// ── Task & Status ───────────────────────────────────────────────────────────

/// The unit of work a caller asks an agent to perform. Tasks accumulate
/// status transitions, messages (`history`), and outputs (`artifacts`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Task {
    /// Always `"task"` on the wire.
    pub kind: TaskKind,
    /// Server-assigned task id.
    pub id: String,
    /// Conversation/context id (sticky across related tasks).
    #[serde(rename = "contextId")]
    pub context_id: String,
    /// Current status (latest state transition).
    pub status: TaskStatus,
    /// Output artifacts (final results).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub artifacts: Vec<Artifact>,
    /// Message history (user + agent turns).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub history: Vec<Message>,
    /// Free-form metadata.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<IndexMap<String, Value>>,
}

/// Discriminator for [`Task`]. Always `"task"`.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum TaskKind {
    /// The only valid variant.
    #[default]
    Task,
}

/// `Task.status` — current state plus an optional message describing the
/// transition (e.g. "agent is asking for more information").
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TaskStatus {
    /// Current state.
    pub state: TaskState,
    /// Optional message explaining the transition. For
    /// [`TaskState::InputRequired`] this carries the agent's question.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<Message>,
    /// ISO 8601 timestamp of the transition.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<String>,
}

/// `TaskStatus.state`. The spec defines a closed set; non-terminal vs
/// terminal classification is encoded in [`TaskState::is_terminal`].
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum TaskState {
    /// Task accepted, not yet started.
    Submitted,
    /// Agent is working on it.
    Working,
    /// Agent paused, awaiting more input from the user.
    InputRequired,
    /// Task completed successfully (terminal).
    Completed,
    /// Task was canceled by caller (terminal).
    Canceled,
    /// Task failed (terminal).
    Failed,
    /// Server refused the task outright (terminal).
    Rejected,
    /// Task is blocked on out-of-band authentication.
    AuthRequired,
    /// State is unknown (e.g. server lost it).
    Unknown,
}

impl TaskState {
    /// True if no further transitions are possible.
    #[must_use]
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Completed | Self::Canceled | Self::Failed | Self::Rejected
        )
    }
}

// ── Artifact ────────────────────────────────────────────────────────────────

/// A typed output attached to a [`Task`].
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Artifact {
    /// Server-assigned id.
    #[serde(rename = "artifactId")]
    pub artifact_id: String,
    /// Optional human name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Optional human description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Parts that make up the artifact body.
    pub parts: Vec<Part>,
    /// If streamed in chunks, the chunk index.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub index: Option<u32>,
    /// If true, append the new parts onto an existing artifact (used during
    /// streaming).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub append: Option<bool>,
    /// If true, this is the final chunk of a streamed artifact.
    #[serde(default, rename = "lastChunk", skip_serializing_if = "Option::is_none")]
    pub last_chunk: Option<bool>,
    /// Free-form metadata.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<IndexMap<String, Value>>,
}

// ── Streaming update events ────────────────────────────────────────────────

/// Streaming notification — task status changed.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TaskStatusUpdateEvent {
    /// Always `"status-update"`.
    pub kind: StatusUpdateKind,
    /// Task id this update applies to.
    #[serde(rename = "taskId")]
    pub task_id: String,
    /// Conversation/context id.
    #[serde(rename = "contextId")]
    pub context_id: String,
    /// New status.
    pub status: TaskStatus,
    /// True if this is the last update on the SSE stream for this task.
    #[serde(rename = "final")]
    pub is_final: bool,
    /// Free-form metadata.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<IndexMap<String, Value>>,
}

/// Discriminator for [`TaskStatusUpdateEvent`].
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum StatusUpdateKind {
    /// The only valid variant.
    #[default]
    StatusUpdate,
}

/// Streaming notification — task gained (or appended to) an artifact.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TaskArtifactUpdateEvent {
    /// Always `"artifact-update"`.
    pub kind: ArtifactUpdateKind,
    /// Task id this update applies to.
    #[serde(rename = "taskId")]
    pub task_id: String,
    /// Conversation/context id.
    #[serde(rename = "contextId")]
    pub context_id: String,
    /// The new artifact (or the chunk being appended).
    pub artifact: Artifact,
    /// Free-form metadata.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<IndexMap<String, Value>>,
}

/// Discriminator for [`TaskArtifactUpdateEvent`].
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum ArtifactUpdateKind {
    /// The only valid variant.
    #[default]
    ArtifactUpdate,
}

/// A streaming SSE result — the `result` field of an [`A2aResponse`] frame
/// emitted on a `message/stream` or `tasks/resubscribe` channel. The
/// variants line up with what the Python A2A server emits.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum StreamingMessageResult {
    /// Task snapshot (initial, or after `final: true`).
    Task(Task),
    /// Standalone message reply (no task created).
    Message(Message),
    /// Status transition.
    Status(TaskStatusUpdateEvent),
    /// Artifact added or appended.
    Artifact(TaskArtifactUpdateEvent),
}

// ── Method params ──────────────────────────────────────────────────────────

/// Params for [`method::MESSAGE_SEND`] / [`method::MESSAGE_STREAM`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageSendParams {
    /// The user message to send.
    pub message: Message,
    /// Optional per-call configuration.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub configuration: Option<MessageSendConfiguration>,
    /// Free-form metadata.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<IndexMap<String, Value>>,
}

/// Per-call configuration accepted by `message/send` and `message/stream`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MessageSendConfiguration {
    /// MIME types the caller is willing to accept on the output side. The
    /// server should reject the call (`-32602`) if it can't produce any of
    /// them.
    #[serde(
        default,
        rename = "acceptedOutputModes",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub accepted_output_modes: Vec<String>,
    /// If true, the server should keep `Task.history` populated rather than
    /// dropping old turns.
    #[serde(
        default,
        rename = "historyLength",
        skip_serializing_if = "Option::is_none"
    )]
    pub history_length: Option<u32>,
    /// If true, the server should stream task state via SSE. Ignored on
    /// `message/send` (which never streams).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocking: Option<bool>,
}

/// Params for [`method::TASKS_GET`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskQueryParams {
    /// Task id to look up.
    pub id: String,
    /// If set, trim `history` to at most this many entries.
    #[serde(
        default,
        rename = "historyLength",
        skip_serializing_if = "Option::is_none"
    )]
    pub history_length: Option<u32>,
}

/// Params for [`method::TASKS_CANCEL`] / [`method::TASKS_RESUBSCRIBE`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskIdParams {
    /// Task id.
    pub id: String,
}

// ── Push notifications ──────────────────────────────────────────────────────

/// Webhook config registered against a task for out-of-band update delivery.
///
/// When set, the server POSTs each [`TaskStatusUpdateEvent`] /
/// [`TaskArtifactUpdateEvent`] to `url` for the lifetime of the task. The
/// body is the same JSON-RPC-style envelope used on `message/stream`:
/// `{"jsonrpc":"2.0","result":<update>}`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PushNotificationConfig {
    /// Server-assigned id. Provided in responses; ignored on `set`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// Destination URL. Must be HTTPS or a loopback host on the server side
    /// (the server enforces this; a config that points at a public
    /// `http://` URL is refused with `INVALID_PARAMS`).
    pub url: String,
    /// Optional bearer token the receiver expects on the inbound webhook.
    /// When set, the server adds `Authorization: Bearer <token>` to each
    /// outbound POST.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
    /// Optional richer authentication descriptor (mirrors the spec's
    /// `PushNotificationAuthenticationInfo`). Reserved for future use; the
    /// server reads `token` first.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub authentication: Option<PushNotificationAuthenticationInfo>,
}

/// Richer authentication descriptor for [`PushNotificationConfig`]. Mirrors
/// the spec but is not yet enforced beyond the `Bearer`/`token` shortcut.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct PushNotificationAuthenticationInfo {
    /// Accepted auth schemes (e.g. `["Bearer"]`).
    pub schemes: Vec<String>,
    /// Optional credentials payload (auth-scheme-specific).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credentials: Option<String>,
}

/// Bundle of `taskId` + a [`PushNotificationConfig`]. Used as both the
/// request params and the response result for the push-config methods.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskPushNotificationConfig {
    /// Task this config applies to.
    #[serde(rename = "taskId")]
    pub task_id: String,
    /// The config itself.
    #[serde(rename = "pushNotificationConfig")]
    pub push_notification_config: PushNotificationConfig,
}

/// Params for [`method::TASKS_PUSH_NOTIFICATION_CONFIG_GET`] /
/// [`method::TASKS_PUSH_NOTIFICATION_CONFIG_DELETE`]. The spec lets these
/// look up a specific config by `pushNotificationConfigId`; if omitted, the
/// server treats it as "all configs for the task" (delete) or "the first
/// registered config" (get) for backwards compatibility.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetTaskPushNotificationConfigParams {
    /// Task id.
    pub id: String,
    /// Specific config id within the task.
    #[serde(
        default,
        rename = "pushNotificationConfigId",
        skip_serializing_if = "Option::is_none"
    )]
    pub push_notification_config_id: Option<String>,
}

/// Result for [`method::TASKS_PUSH_NOTIFICATION_CONFIG_LIST`].
pub type ListTaskPushNotificationConfigResult = Vec<TaskPushNotificationConfig>;

// ── Agent card ──────────────────────────────────────────────────────────────

/// The Agent Card — served at `/.well-known/agent.json` (configurable) so
/// clients can discover what an agent can do without dispatching a real
/// task.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AgentCard {
    /// Human-readable agent name.
    pub name: String,
    /// Short description.
    pub description: String,
    /// JSON-RPC endpoint URL.
    pub url: String,
    /// Agent provider (vendor) info, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<AgentProvider>,
    /// Agent version string.
    pub version: String,
    /// Optional URL pointing at richer docs.
    #[serde(
        default,
        rename = "documentationUrl",
        skip_serializing_if = "Option::is_none"
    )]
    pub documentation_url: Option<String>,
    /// Capability flags the server claims to support.
    pub capabilities: AgentCapabilities,
    /// Authentication schemes accepted at `url`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub authentication: Option<AgentAuthentication>,
    /// MIME types the agent accepts as input by default.
    #[serde(default, rename = "defaultInputModes")]
    pub default_input_modes: Vec<String>,
    /// MIME types the agent produces by default.
    #[serde(default, rename = "defaultOutputModes")]
    pub default_output_modes: Vec<String>,
    /// Declared agent skills.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skills: Vec<AgentSkill>,
}

/// Capability flags advertised in [`AgentCard`].
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentCapabilities {
    /// Supports `message/stream` and `tasks/resubscribe`.
    #[serde(default)]
    pub streaming: bool,
    /// Supports `tasks/pushNotificationConfig/*`.
    #[serde(default, rename = "pushNotifications")]
    pub push_notifications: bool,
    /// Maintains state history across reconnects.
    #[serde(default, rename = "stateTransitionHistory")]
    pub state_transition_history: bool,
}

/// `AgentCard.provider`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentProvider {
    /// Organization name.
    pub organization: String,
    /// Vendor URL.
    pub url: String,
}

/// `AgentCard.authentication`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentAuthentication {
    /// List of accepted auth schemes (e.g. `["Bearer"]`).
    pub schemes: Vec<String>,
    /// Optional credentials hint (e.g. OAuth2 issuer URL).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credentials: Option<String>,
}

/// One declared skill the agent offers.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AgentSkill {
    /// Stable skill id.
    pub id: String,
    /// Human name.
    pub name: String,
    /// Short description.
    pub description: String,
    /// Tags / categories.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    /// Worked examples (free-form strings).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub examples: Vec<String>,
    /// MIME types accepted by this skill specifically (overrides
    /// [`AgentCard::default_input_modes`]).
    #[serde(default, rename = "inputModes", skip_serializing_if = "Vec::is_empty")]
    pub input_modes: Vec<String>,
    /// MIME types produced by this skill specifically.
    #[serde(default, rename = "outputModes", skip_serializing_if = "Vec::is_empty")]
    pub output_modes: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn part_text_serializes_with_kind_tag() {
        let p = Part::text("hello");
        let v = serde_json::to_value(&p).unwrap();
        assert_eq!(v, json!({"kind": "text", "text": "hello"}));
        let back: Part = serde_json::from_value(v).unwrap();
        assert_eq!(p, back);
    }

    #[test]
    fn file_part_round_trips_inline_and_uri() {
        let inline = Part::File {
            file: FilePayload::Inline {
                name: Some("a.txt".into()),
                mime_type: Some("text/plain".into()),
                bytes: "aGk=".into(),
            },
            metadata: None,
        };
        let v = serde_json::to_value(&inline).unwrap();
        assert_eq!(v["kind"], "file");
        assert_eq!(v["file"]["bytes"], "aGk=");
        let back: Part = serde_json::from_value(v).unwrap();
        assert_eq!(inline, back);

        let uri = Part::File {
            file: FilePayload::Uri {
                name: None,
                mime_type: Some("image/png".into()),
                uri: "https://example.com/x.png".into(),
            },
            metadata: None,
        };
        let v = serde_json::to_value(&uri).unwrap();
        assert_eq!(v["file"]["uri"], "https://example.com/x.png");
        let back: Part = serde_json::from_value(v).unwrap();
        assert_eq!(uri, back);
    }

    #[test]
    fn message_serializes_message_id_and_role() {
        let m = Message::user_text("hi");
        let v = serde_json::to_value(&m).unwrap();
        assert_eq!(v["role"], "user");
        assert_eq!(v["kind"], "message");
        assert!(v["messageId"].is_string());
        assert_eq!(v["parts"][0]["kind"], "text");
        let back: Message = serde_json::from_value(v).unwrap();
        assert_eq!(m, back);
    }

    #[test]
    fn task_kind_field_round_trips() {
        let t = Task {
            kind: TaskKind::Task,
            id: "t-1".into(),
            context_id: "c-1".into(),
            status: TaskStatus {
                state: TaskState::Working,
                message: None,
                timestamp: None,
            },
            artifacts: vec![],
            history: vec![],
            metadata: None,
        };
        let v = serde_json::to_value(&t).unwrap();
        assert_eq!(v["kind"], "task");
        assert_eq!(v["status"]["state"], "working");
        let back: Task = serde_json::from_value(v).unwrap();
        assert_eq!(t, back);
    }

    #[test]
    fn task_state_terminality() {
        assert!(TaskState::Completed.is_terminal());
        assert!(TaskState::Canceled.is_terminal());
        assert!(TaskState::Failed.is_terminal());
        assert!(TaskState::Rejected.is_terminal());
        assert!(!TaskState::Working.is_terminal());
        assert!(!TaskState::InputRequired.is_terminal());
        assert!(!TaskState::Submitted.is_terminal());
    }

    #[test]
    fn input_required_state_serializes_as_kebab() {
        let v = serde_json::to_value(TaskState::InputRequired).unwrap();
        assert_eq!(v, json!("input-required"));
    }

    #[test]
    fn streaming_event_kinds_are_kebab() {
        let st = TaskStatusUpdateEvent {
            kind: StatusUpdateKind::StatusUpdate,
            task_id: "t".into(),
            context_id: "c".into(),
            status: TaskStatus {
                state: TaskState::Working,
                message: None,
                timestamp: None,
            },
            is_final: false,
            metadata: None,
        };
        let v = serde_json::to_value(&st).unwrap();
        assert_eq!(v["kind"], "status-update");
        assert_eq!(v["final"], false);
        let back: TaskStatusUpdateEvent = serde_json::from_value(v).unwrap();
        assert_eq!(st, back);
    }

    #[test]
    fn streaming_result_dispatches_to_correct_variant() {
        let v = json!({
            "kind": "task",
            "id": "t-1",
            "contextId": "c-1",
            "status": {"state": "working"}
        });
        match serde_json::from_value::<StreamingMessageResult>(v).unwrap() {
            StreamingMessageResult::Task(t) => assert_eq!(t.id, "t-1"),
            other => panic!("expected Task, got {other:?}"),
        }

        let v = json!({
            "kind": "status-update",
            "taskId": "t-1",
            "contextId": "c-1",
            "status": {"state": "completed"},
            "final": true
        });
        match serde_json::from_value::<StreamingMessageResult>(v).unwrap() {
            StreamingMessageResult::Status(s) => assert!(s.is_final),
            other => panic!("expected Status, got {other:?}"),
        }
    }

    #[test]
    fn push_config_round_trips() {
        let cfg = PushNotificationConfig {
            id: Some("pn-1".into()),
            url: "https://hooks.example.com/cb".into(),
            token: Some("secret".into()),
            authentication: None,
        };
        let v = serde_json::to_value(&cfg).unwrap();
        assert_eq!(v["url"], "https://hooks.example.com/cb");
        assert_eq!(v["token"], "secret");
        let back: PushNotificationConfig = serde_json::from_value(v).unwrap();
        assert_eq!(cfg, back);
    }

    #[test]
    fn task_push_config_bundle_uses_spec_field_names() {
        let bundle = TaskPushNotificationConfig {
            task_id: "t-1".into(),
            push_notification_config: PushNotificationConfig {
                id: None,
                url: "https://hooks.example.com/x".into(),
                token: None,
                authentication: None,
            },
        };
        let v = serde_json::to_value(&bundle).unwrap();
        assert_eq!(v["taskId"], "t-1");
        assert!(v["pushNotificationConfig"].is_object());
    }

    #[test]
    fn agent_card_round_trips_minimal() {
        let card = AgentCard {
            name: "Greeter".into(),
            description: "Says hi".into(),
            url: "https://example.com/a2a".into(),
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
        };
        let v = serde_json::to_value(&card).unwrap();
        assert_eq!(v["capabilities"]["streaming"], true);
        assert_eq!(v["defaultInputModes"][0], "text/plain");
        let back: AgentCard = serde_json::from_value(v).unwrap();
        assert_eq!(card, back);
    }
}
