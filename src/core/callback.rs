//! Type aliases for the user-provided callbacks that hook into the agent
//! lifecycle. All callbacks are boxed async closures stored as `Arc<dyn Fn>`
//! so they can be cloned cheaply and stored in heterogeneous agent vectors.

use std::sync::Arc;

use futures::future::BoxFuture;
use serde_json::Value;

use crate::error::Result;
use crate::genai_types::Content;

use crate::core::context::{InvocationContext, ToolContext};
use crate::core::llm_request::LlmRequest;
use crate::core::llm_response::LlmResponse;
use crate::core::tool_object::DynTool;

/// Read-only view of the invocation context, passed to instruction providers.
#[derive(Clone)]
pub struct ReadonlyContext {
    /// Underlying invocation context.
    pub invocation: Arc<InvocationContext>,
}

impl ReadonlyContext {
    /// Construct.
    #[must_use]
    pub fn new(invocation: Arc<InvocationContext>) -> Self {
        Self { invocation }
    }
}

/// A mutable callback context — same as [`InvocationContext`] but boxed for
/// callbacks (a future revision may add per-callback mutation helpers).
#[derive(Clone)]
pub struct CallbackContext {
    /// Underlying invocation context.
    pub invocation: Arc<InvocationContext>,
}

impl CallbackContext {
    /// Construct.
    #[must_use]
    pub fn new(invocation: Arc<InvocationContext>) -> Self {
        Self { invocation }
    }
}

/// `before_agent_callback`: return `Some(content)` to short-circuit the agent
/// and return that content as its sole response.
pub type BeforeAgentCallback = Arc<
    dyn for<'a> Fn(&'a mut CallbackContext) -> BoxFuture<'a, Result<Option<Content>>> + Send + Sync,
>;

/// `after_agent_callback`: return `Some(content)` to replace the agent's
/// emitted content with the returned content.
pub type AfterAgentCallback = BeforeAgentCallback;

/// `before_model_callback`: optionally mutate the outgoing request or
/// short-circuit with a synthetic response.
pub type BeforeModelCallback = Arc<
    dyn for<'a> Fn(
            &'a mut CallbackContext,
            &'a mut LlmRequest,
        ) -> BoxFuture<'a, Result<Option<LlmResponse>>>
        + Send
        + Sync,
>;

/// `after_model_callback`: optionally rewrite the response.
pub type AfterModelCallback = Arc<
    dyn for<'a> Fn(
            &'a mut CallbackContext,
            &'a mut LlmResponse,
        ) -> BoxFuture<'a, Result<Option<LlmResponse>>>
        + Send
        + Sync,
>;

/// `on_model_error_callback`: see Python `OnModelErrorCallback`.
pub type OnModelErrorCallback = Arc<
    dyn for<'a> Fn(
            &'a mut CallbackContext,
            &'a mut LlmRequest,
            &'a crate::error::Error,
        ) -> BoxFuture<'a, Result<Option<LlmResponse>>>
        + Send
        + Sync,
>;

/// `before_tool_callback`: return `Some(result)` to short-circuit tool exec.
pub type BeforeToolCallback = Arc<
    dyn for<'a> Fn(
            &'a mut ToolContext,
            &'a Arc<dyn DynTool>,
            &'a mut Value,
        ) -> BoxFuture<'a, Result<Option<Value>>>
        + Send
        + Sync,
>;

/// `after_tool_callback`: optionally rewrite the tool result.
pub type AfterToolCallback = Arc<
    dyn for<'a> Fn(
            &'a mut ToolContext,
            &'a Arc<dyn DynTool>,
            &'a Value,
            &'a mut Value,
        ) -> BoxFuture<'a, Result<Option<Value>>>
        + Send
        + Sync,
>;

/// `on_tool_error_callback`: optional recovery hook.
pub type OnToolErrorCallback = Arc<
    dyn for<'a> Fn(
            &'a mut ToolContext,
            &'a Arc<dyn DynTool>,
            &'a Value,
            &'a crate::error::Error,
        ) -> BoxFuture<'a, Result<Option<Value>>>
        + Send
        + Sync,
>;

/// Macro that turns a regular `async fn` (or closure that returns a future)
/// into a [`BeforeAgentCallback`]-shaped value.
///
/// Usage:
/// ```ignore
/// let cb = crate::core::before_agent_callback!(|ctx| async move {
///     Ok(None)
/// });
/// ```
#[macro_export]
macro_rules! before_agent_callback {
    ($f:expr) => {{
        let f = $f;
        ::std::sync::Arc::new(
            move |ctx: &mut $crate::core::callback::CallbackContext| -> ::futures::future::BoxFuture<
                '_,
                $crate::core::Result<::std::option::Option<$crate::core::types::Content>>,
            > { ::std::boxed::Box::pin((f)(ctx)) },
        ) as $crate::core::callback::BeforeAgentCallback
    }};
}
