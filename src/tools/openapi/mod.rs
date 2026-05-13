//! OpenAPI tool generator (`feature = "openapi"`).
//!
//! Point [`OpenAPIToolset`] at an OpenAPI 3.x spec and get a
//! `Vec<Arc<dyn Tool>>` back, one per operation in the spec. Security schemes
//! from the spec map to [`crate::auth::AuthConfig`] entries so the runner
//! resolves credentials before dispatching each tool.

mod operation;
mod rest_tool;
mod spec;
mod toolset;

pub use operation::{ApiParameter, HttpMethod, ParamLocation, ParsedOperation, to_snake_case};
pub use rest_tool::RestApiTool;
pub use spec::{SpecParse, parse_spec};
pub use toolset::OpenAPIToolset;
