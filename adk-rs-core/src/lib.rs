//! Core types and service traits for adk-rs.
//!
//! This crate is the domain layer: it owns `Event`, `Session`, `State`,
//! `LlmRequest`/`LlmResponse`, `InvocationContext`, and the service traits
//! (`SessionService`, `ArtifactService`, `MemoryService`, `CredentialService`,
//! `Model`). Implementations live in `adk-services-*` and `adk-providers-*`.

#![forbid(unsafe_code)]
#![deny(missing_docs)]

pub mod artifact;
pub mod callback;
pub mod context;
pub mod event;
pub mod llm_request;
pub mod llm_response;
pub mod memory;
pub mod model;
pub mod run_config;
pub mod services;
pub mod session;
pub mod state;
pub mod stream;
pub mod tool_object;

#[cfg(any(test, feature = "testing"))]
pub mod testing;

pub use adk_rs_error::{
    Context as ErrorContext, Error, ProviderError, Result, ServiceError, ToolError,
};
pub use adk_rs_genai_types as types;
pub use artifact::{Artifact, ArtifactKey};
pub use callback::{
    AfterAgentCallback, AfterModelCallback, AfterToolCallback, BeforeAgentCallback,
    BeforeModelCallback, BeforeToolCallback, CallbackContext, OnModelErrorCallback,
    OnToolErrorCallback, ReadonlyContext,
};
pub use context::{InvocationContext, InvocationOrigin, ToolContext};
pub use event::{Event, EventActions, EventCompaction};
pub use llm_request::LlmRequest;
pub use llm_response::LlmResponse;
pub use memory::{MemoryEntry, SearchMemoryResponse};
pub use model::{Model, ModelRegistry};
pub use run_config::{RunConfig, StreamingMode};
pub use services::{
    ArtifactService, CredentialService, GetSessionConfig, ListSessionsResponse, MemoryService,
    SessionService, SessionsMeta,
};
pub use session::{Session, SessionId, SessionMeta};
pub use state::{State, StateDelta, StateScope};
pub use stream::{EventStream, LlmResponseStream};
pub use tool_object::DynTool;
