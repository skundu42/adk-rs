//! [`FunctionTool`] — wrap an `async fn(args, ctx) -> Result<Value>` into a
//! [`Tool`](crate::tools::Tool).
//!
//! The high-ergonomics form is the `#[adk::tool]` proc-macro in
//! `adk-tools-macros`; `FunctionTool::new` is the explicit fallback.

use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use futures::future::BoxFuture;
use serde_json::Value;

use crate::core::{DynTool, ToolContext};
use crate::error::Result;
use crate::genai_types::{FunctionDeclaration, Schema};

/// Function signature accepted by [`FunctionTool::new`].
pub(crate) type FunctionToolFn = Arc<
    dyn for<'a> Fn(Value, &'a mut ToolContext) -> BoxFuture<'a, Result<Value>>
        + Send
        + Sync
        + 'static,
>;

/// A tool wrapping a user-provided async closure.
pub struct FunctionTool {
    name: String,
    description: String,
    parameters: Option<Schema>,
    long_running: bool,
    f: FunctionToolFn,
}

impl std::fmt::Debug for FunctionTool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FunctionTool")
            .field("name", &self.name)
            .field("description", &self.description)
            .field("long_running", &self.long_running)
            .finish_non_exhaustive()
    }
}

impl FunctionTool {
    /// Construct.
    pub fn new(
        name: impl Into<String>,
        description: impl Into<String>,
        parameters: Option<Schema>,
        f: FunctionToolFn,
    ) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            parameters,
            long_running: false,
            f,
        }
    }

    /// Mark the tool as long-running.
    #[must_use]
    pub fn with_long_running(mut self, yes: bool) -> Self {
        self.long_running = yes;
        self
    }

    /// Wrap any `async fn(Value, &mut ToolContext) -> Result<Value>`.
    pub fn from_async<F, Fut>(
        name: impl Into<String>,
        description: impl Into<String>,
        parameters: Option<Schema>,
        f: F,
    ) -> Self
    where
        F: for<'a> Fn(Value, &'a mut ToolContext) -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = Result<Value>> + Send + 'static,
    {
        let f = Arc::new(f);
        let boxed: FunctionToolFn = Arc::new(move |v, ctx| {
            let f = f.clone();
            let fut = f(v, ctx);
            Box::pin(fut) as Pin<Box<dyn std::future::Future<Output = _> + Send>>
        });
        Self::new(name, description, parameters, boxed)
    }
}

#[async_trait]
impl DynTool for FunctionTool {
    fn name(&self) -> &str {
        &self.name
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn is_long_running(&self) -> bool {
        self.long_running
    }

    fn declaration(&self) -> Option<FunctionDeclaration> {
        Some(
            FunctionDeclaration::new(&self.name, &self.description)
                .with_parameters(self.parameters.clone().unwrap_or_else(Schema::object)),
        )
    }

    async fn run(&self, args: Value, ctx: &mut ToolContext) -> Result<Value> {
        (self.f)(args, ctx).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    use crate::core::{InvocationContext, InvocationOrigin, RunConfig};
    use adk_services_mem_for_tests::dummy_invocation;
    use serde_json::json;

    #[tokio::test]
    async fn echo_tool_runs() {
        let t = FunctionTool::from_async(
            "echo",
            "echo the args",
            Some(
                Schema::object()
                    .property("msg", Schema::string())
                    .require("msg"),
            ),
            |args: Value, _ctx: &mut ToolContext| async move { Ok(args) },
        );
        let inv = dummy_invocation();
        let mut ctx = ToolContext::new(Arc::new(inv));
        let r = t.run(json!({"msg": "hi"}), &mut ctx).await.unwrap();
        assert_eq!(r["msg"], "hi");
        assert!(t.declaration().unwrap().parameters.is_some());
    }

    // Helper: produce a dummy invocation context without spinning up real
    // services. Lives in a private inline module so it does not leak.
    mod adk_services_mem_for_tests {
        use super::*;

        pub(super) fn dummy_invocation() -> InvocationContext {
            use parking_lot::Mutex;
            use std::collections::HashMap;
            use std::sync::Arc;

            #[derive(Debug)]
            struct NoopSession;
            #[async_trait]
            impl crate::core::SessionService for NoopSession {
                async fn create_session(
                    &self,
                    app: &str,
                    user: &str,
                    _state: Option<crate::core::State>,
                    id: Option<&str>,
                ) -> crate::error::Result<crate::core::Session> {
                    Ok(crate::core::Session::new(app, user, id.unwrap_or("s")))
                }
                async fn get_session(
                    &self,
                    _: &str,
                    _: &str,
                    _: &str,
                    _: crate::core::GetSessionConfig,
                ) -> crate::error::Result<Option<crate::core::Session>> {
                    Ok(None)
                }
                async fn list_sessions(
                    &self,
                    _: &str,
                    _: &str,
                ) -> crate::error::Result<crate::core::ListSessionsResponse> {
                    Ok(crate::core::ListSessionsResponse::default())
                }
                async fn delete_session(
                    &self,
                    _: &str,
                    _: &str,
                    _: &str,
                ) -> crate::error::Result<()> {
                    Ok(())
                }
            }

            InvocationContext {
                app_name: "app".into(),
                user_id: "u".into(),
                invocation_id: "inv-1".into(),
                session: Arc::new(Mutex::new(crate::core::Session::new("app", "u", "s"))),
                session_service: Arc::new(NoopSession),
                artifact_service: None,
                memory_service: None,
                credential_service: None,
                run_config: RunConfig::default(),
                origin: InvocationOrigin::Api,
                user_content: None,
                llm_call_count: Arc::new(Mutex::new(0)),
                attributes: Arc::new(Mutex::new(HashMap::new())),
            }
        }
    }
}
