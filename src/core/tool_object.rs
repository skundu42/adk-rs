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
    ///
    /// When `true`, the agent emits the tool's `FunctionResponse` with
    /// `will_continue: Some(true)`. The caller is responsible for resubmitting
    /// the final result by including a fresh `FunctionResponse` on a
    /// subsequent invocation.
    fn is_long_running(&self) -> bool {
        false
    }

    /// Auth requirements for this tool. When `Some`, the runner resolves the
    /// credential via [`crate::auth::CredentialManager`] *before* calling
    /// [`Self::run`], and injects the result into
    /// [`crate::core::ToolContext::auth_credential`]. When the underlying
    /// flow needs interactive consent the runner emits an
    /// `adk_request_credential` function-call event instead of dispatching
    /// the tool.
    fn auth_config(&self) -> Option<&crate::auth::AuthConfig> {
        None
    }

    /// Whether this call requires explicit user confirmation before it can
    /// run (human-in-the-loop). When `true`, the agent pauses with an
    /// `adk_request_confirmation` request instead of dispatching the tool;
    /// see [`crate::core::tool_confirmation`]. `args` lets implementations
    /// decide per call (e.g. only confirm destructive operations).
    fn requires_confirmation(&self, _args: &Value) -> bool {
        false
    }

    /// Human-readable hint shown to the user when confirmation is
    /// requested. Defaults to a generic message naming the tool.
    fn confirmation_hint(&self, _args: &Value) -> String {
        format!("Approve execution of tool `{}`?", self.name())
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
