//! Integration test for the #[adk::tool] proc-macro.
#![allow(clippy::unwrap_used)]

use adk_rs_core::ToolContext;
use adk_rs_error::Result;
use adk_rs_tools::tool;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;

#[derive(Deserialize, JsonSchema)]
struct AddArgs {
    /// First addend
    a: i32,
    /// Second addend
    b: i32,
}

#[derive(Serialize)]
struct AddResult {
    sum: i32,
}

/// Adds two integers.
#[tool]
async fn add(args: AddArgs, _ctx: &mut ToolContext) -> Result<AddResult> {
    Ok(AddResult {
        sum: args.a + args.b,
    })
}

#[tokio::test]
async fn macro_emits_working_tool() {
    use adk_rs_core::{InvocationContext, InvocationOrigin, RunConfig, Session};
    use parking_lot::Mutex;
    use std::collections::HashMap;
    use std::sync::Arc;

    #[derive(Debug)]
    struct NoopSession;
    #[async_trait::async_trait]
    impl adk_rs_core::SessionService for NoopSession {
        async fn create_session(
            &self,
            app: &str,
            user: &str,
            _: Option<adk_rs_core::State>,
            id: Option<&str>,
        ) -> Result<Session> {
            Ok(Session::new(app, user, id.unwrap_or("s")))
        }
        async fn get_session(
            &self,
            _: &str,
            _: &str,
            _: &str,
            _: adk_rs_core::GetSessionConfig,
        ) -> Result<Option<Session>> {
            Ok(None)
        }
        async fn list_sessions(&self, _: &str, _: &str) -> Result<adk_rs_core::ListSessionsResponse> {
            Ok(adk_rs_core::ListSessionsResponse::default())
        }
        async fn delete_session(&self, _: &str, _: &str, _: &str) -> Result<()> {
            Ok(())
        }
    }

    let inv = Arc::new(InvocationContext {
        app_name: "a".into(),
        user_id: "u".into(),
        invocation_id: "inv".into(),
        session: Arc::new(Mutex::new(Session::new("a", "u", "s"))),
        session_service: Arc::new(NoopSession),
        artifact_service: None,
        memory_service: None,
        credential_service: None,
        run_config: RunConfig::default(),
        origin: InvocationOrigin::Api,
        user_content: None,
        llm_call_count: Arc::new(Mutex::new(0)),
        attributes: Arc::new(Mutex::new(HashMap::new())),
    });

    let t = add();
    assert_eq!(t.name(), "add");
    assert!(t.description().contains("Adds two integers"));
    let decl = t.declaration().unwrap();
    assert_eq!(decl.name, "add");
    assert!(decl.parameters.is_some());

    let mut ctx = ToolContext::new(inv);
    let r = t.run(json!({"a": 2, "b": 3}), &mut ctx).await.unwrap();
    assert_eq!(r["sum"], 5);
}
