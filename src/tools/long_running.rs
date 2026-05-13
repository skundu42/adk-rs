//! [`LongRunningFunctionTool`] — wraps an arbitrary async function and marks
//! it as long-running. The agent emits the returned value as a `pending`
//! response and yields the stream; the caller resumes by submitting the
//! eventual result as a follow-up `FunctionResponse`.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;

use crate::core::{DynTool, ToolContext};
use crate::error::Result;
use crate::genai_types::{FunctionDeclaration, Schema};

type Handler = Arc<
    dyn Fn(Value, &mut ToolContext) -> futures::future::BoxFuture<'static, Result<Value>>
        + Send
        + Sync
        + 'static,
>;

/// Long-running wrapper around an async function.
pub struct LongRunningFunctionTool {
    name: String,
    description: String,
    schema: Option<Schema>,
    handler: Handler,
}

impl std::fmt::Debug for LongRunningFunctionTool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LongRunningFunctionTool")
            .field("name", &self.name)
            .field("description", &self.description)
            .finish_non_exhaustive()
    }
}

impl LongRunningFunctionTool {
    /// Build from a handler closure.
    pub fn new<F, Fut>(
        name: impl Into<String>,
        description: impl Into<String>,
        schema: Option<Schema>,
        handler: F,
    ) -> Arc<Self>
    where
        F: Fn(Value, &mut ToolContext) -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = Result<Value>> + Send + 'static,
    {
        let handler: Handler = Arc::new(move |args, ctx| Box::pin(handler(args, ctx)));
        Arc::new(Self {
            name: name.into(),
            description: description.into(),
            schema,
            handler,
        })
    }
}

#[async_trait]
impl DynTool for LongRunningFunctionTool {
    fn name(&self) -> &str {
        &self.name
    }
    fn description(&self) -> &str {
        &self.description
    }
    fn is_long_running(&self) -> bool {
        true
    }
    fn declaration(&self) -> Option<FunctionDeclaration> {
        Some({
            let d = FunctionDeclaration::new(&self.name, &self.description);
            match &self.schema {
                Some(s) => d.with_parameters(s.clone()),
                None => d,
            }
        })
    }
    async fn run(&self, args: Value, ctx: &mut ToolContext) -> Result<Value> {
        ctx.long_running = true;
        (self.handler)(args, ctx).await
    }
}
