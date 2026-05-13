//! Wire-neutral generative-AI types used throughout adk-rs.
//!
//! These types mirror Google's `google.genai.types` Pydantic models closely
//! enough that JSON IO is interoperable with the Python ADK, but are owned
//! by this crate (no provider SDK dependency). Each provider client
//! translates these into its own wire format.


pub mod content;
pub mod function;
pub mod generate_content_config;
pub mod part;
pub mod response;
pub mod role;
pub mod schema;
pub mod tool;

pub use content::Content;
pub use function::{FunctionCall, FunctionResponse, Scheduling};
pub use generate_content_config::{
    GenerateContentConfig, HarmBlockThreshold, HarmCategory, SafetySetting, ThinkingConfig,
    ToolConfig, ToolMode,
};
pub use part::{CodeExecutionResult, ExecutableCode, FileData, InlineData, Outcome, Part};
pub use response::{
    Candidate, CitationMetadata, FinishReason, GenerateContentResponse, GroundingMetadata,
    PromptFeedback, SafetyRating, UsageMetadata,
};
pub use role::Role;
pub use schema::{Schema, SchemaType};
pub use tool::{FunctionDeclaration, Tool};
