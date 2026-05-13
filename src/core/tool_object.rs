//! The [`DynTool`] trait — the runtime-dispatch shape of a tool.
//!
//! Concrete `Tool` implementations live in `adk-tools`. We define the
//! minimal trait here so [`crate::core::llm_request::LlmRequest`] can carry a
//! `tools_dict: HashMap<String, Arc<dyn DynTool>>` without depending on
//! `adk-tools`. `adk-tools::Tool` is a re-export of [`DynTool`].

use async_trait::async_trait;
use serde_json::Value;

use crate::error::Result;
use crate::genai_types::FunctionDeclaration;

use crate::core::context::ToolContext;
use crate::core::llm_request::LlmRequest;

/// Runtime-dispatch tool trait.
#[async_trait]
pub trait DynTool: Send + Sync + std::fmt::Debug + 'static {
    /// The tool name (matches the model's function-call name).
    fn name(&self) -> &str;

    /// Human description.
    fn description(&self) -> &str;

    /// Whether this tool yields a long-running operation.
    fn is_long_running(&self) -> bool {
        false
    }

    /// JSON-Schema declaration of the tool's parameters; `None` for tools
    /// (e.g. Gemini built-ins) that should not be advertised to the model.
    fn declaration(&self) -> Option<FunctionDeclaration>;

    /// Execute the tool with JSON args.
    async fn run(&self, args: Value, ctx: &mut ToolContext) -> Result<Value>;

    /// Hook called before the request is sent. Default: append the tool's
    /// declaration into `req.config.tools`.
    async fn process_llm_request(
        &self,
        req: &mut LlmRequest,
        _ctx: &mut ToolContext,
    ) -> Result<()> {
        if let Some(d) = self.declaration() {
            req.append_function_declarations([d]);
        }
        Ok(())
    }
}
