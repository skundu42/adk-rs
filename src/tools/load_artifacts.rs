//! `load_artifacts` built-in. Looks up artifact filenames in the configured
//! [`ArtifactService`](crate::core::ArtifactService) and returns their content
//! as parts.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;

use crate::core::{DynTool, ToolContext};
use crate::error::{Error, Result};
use crate::genai_types::{FunctionDeclaration, Schema};

/// Tool that the model can call to pull one or more artifacts into the
/// conversation. Returns a JSON object: `{ "loaded": [{"name": "...", "kind":
/// "text|binary", "text"?: "...", "mime_type"?: "..." }] }`.
#[derive(Debug)]
struct LoadArtifacts;

#[async_trait]
impl DynTool for LoadArtifacts {
    fn name(&self) -> &str {
        "load_artifacts"
    }
    fn description(&self) -> &str {
        "Load one or more previously-saved artifacts into the conversation. \
         Use this when you need to reference a file the user uploaded earlier."
    }
    fn declaration(&self) -> Option<FunctionDeclaration> {
        Some(
            FunctionDeclaration::new(self.name(), self.description()).with_parameters(
                Schema::object()
                    .property(
                        "artifact_names",
                        Schema::array(Schema::string())
                            .with_description("Artifact filenames to load."),
                    )
                    .require("artifact_names"),
            ),
        )
    }
    async fn run(&self, args: Value, ctx: &mut ToolContext) -> Result<Value> {
        let names: Vec<String> = args
            .get("artifact_names")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .ok_or_else(|| Error::invalid_input("artifact_names must be an array of strings"))?;
        let mut loaded = Vec::with_capacity(names.len());
        for name in names {
            match ctx.load_artifact(&name, None).await? {
                Some(part) => loaded.push(serde_json::json!({
                    "name": name,
                    "part": serde_json::to_value(&part).unwrap_or(Value::Null),
                })),
                None => loaded.push(serde_json::json!({
                    "name": name,
                    "error": "not found",
                })),
            }
        }
        Ok(serde_json::json!({"loaded": loaded}))
    }
}

/// Construct the `load_artifacts` tool.
#[must_use]
pub fn load_artifacts_tool() -> Arc<dyn DynTool> {
    Arc::new(LoadArtifacts)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::ArtifactService;
    use crate::core::artifact::ArtifactKey;
    use crate::genai_types::Part;
    use crate::services::mem::InMemoryArtifactService;
    use serde_json::json;

    fn ctx_with_artifacts(artifacts: Arc<dyn ArtifactService>) -> ToolContext {
        let mut inv = crate::core::testing::test_invocation_context();
        inv.artifact_service = Some(artifacts);
        ToolContext::new(Arc::new(inv))
    }

    #[tokio::test]
    async fn loads_existing_artifact() {
        let artifacts: Arc<dyn ArtifactService> = Arc::new(InMemoryArtifactService::default());
        artifacts
            .save_artifact(
                ArtifactKey::new("app", "u", "s", "hello.txt"),
                Part::text("hello world"),
            )
            .await
            .unwrap();
        let mut tctx = ctx_with_artifacts(artifacts);
        let out = load_artifacts_tool()
            .run(json!({"artifact_names": ["hello.txt"]}), &mut tctx)
            .await
            .unwrap();
        let loaded = &out["loaded"][0];
        assert_eq!(loaded["name"], "hello.txt");
        assert!(loaded["part"].get("text").is_some());
    }

    #[tokio::test]
    async fn reports_missing_artifact() {
        let artifacts: Arc<dyn ArtifactService> = Arc::new(InMemoryArtifactService::default());
        let mut tctx = ctx_with_artifacts(artifacts);
        let out = load_artifacts_tool()
            .run(json!({"artifact_names": ["nope.txt"]}), &mut tctx)
            .await
            .unwrap();
        assert_eq!(out["loaded"][0]["error"], "not found");
    }
}
