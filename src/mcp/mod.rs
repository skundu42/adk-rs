//! MCP (Model Context Protocol) client + [`McpToolset`].
//!
//! Supports two transports:
//!
//! - **stdio** — spawn an MCP server as a child process, talk newline-delimited
//!   JSON-RPC over stdin/stdout. The classic local-server form.
//! - **streamable HTTP** — POST JSON-RPC to a single HTTP endpoint. Responses
//!   come back as either `application/json` (single result) or
//!   `text/event-stream` (SSE). Mirrors the MCP 2025-03 spec onward.

mod client;
mod http;
mod stdio;
mod tool;
mod toolset;
mod transport;

pub use client::{McpClient, McpToolDescriptor};
pub use http::{HttpTransport, McpHttpParams};
pub use stdio::{McpStdioParams, StdioTransport};
pub use tool::McpTool;
pub use toolset::McpToolset;
pub use transport::Transport;
