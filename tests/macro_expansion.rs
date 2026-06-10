//! End-to-end test for the `#[adk_rs::tool]` proc-macro:
//! the generated tool round-trips through JSON serialization and produces
//! the expected `FunctionDeclaration`.

#![cfg(all(feature = "macros", feature = "testing"))]

use std::sync::Arc;

use adk_rs::Tool;
use adk_rs::core::ToolContext;
use adk_rs::tool;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;

#[derive(Deserialize, JsonSchema)]
struct EchoArgs {
    /// The message to echo.
    msg: String,
}

#[derive(Serialize)]
struct EchoOut {
    echoed: String,
}

/// Returns its `msg` argument unchanged.
#[tool]
async fn echo(args: EchoArgs, _ctx: &mut ToolContext) -> adk_rs::Result<EchoOut> {
    Ok(EchoOut { echoed: args.msg })
}

fn ctx() -> ToolContext {
    ToolContext::new(Arc::new(adk_rs::core::testing::test_invocation_context()))
}

#[tokio::test]
async fn macro_generated_tool_name_matches_fn_name() {
    let t = echo();
    assert_eq!(<dyn Tool>::name(&*t), "echo");
    let desc = <dyn Tool>::description(&*t);
    assert!(
        desc.to_lowercase().contains("echo") || desc.contains("Returns"),
        "description should mirror the doc comment; got {desc:?}"
    );
}

#[tokio::test]
async fn macro_generated_declaration_has_schema() {
    let t = echo();
    let decl = t.declaration().expect("declaration should be present");
    assert_eq!(decl.name, "echo");
    assert!(decl.parameters.is_some(), "schema derived from args struct");
}

#[tokio::test]
async fn macro_generated_run_round_trips_args() {
    let t = echo();
    let mut c = ctx();
    let out = t.run(json!({"msg": "ping"}), &mut c).await.unwrap();
    assert_eq!(out["echoed"], "ping");
}

#[tokio::test]
async fn macro_generated_run_rejects_bad_args() {
    let t = echo();
    let mut c = ctx();
    let err = t.run(json!({"msg": 42}), &mut c).await.unwrap_err();
    let s = err.to_string().to_lowercase();
    assert!(
        s.contains("invalid") || s.contains("invalidargs") || s.contains("expected"),
        "got {s}"
    );
}
