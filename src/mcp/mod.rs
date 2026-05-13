//! MCP (Model Context Protocol) stdio client + [`McpToolset`].
//!
//! Spawns an MCP server as a child process, talks newline-delimited JSON-RPC
//! over stdin/stdout, and exposes discovered tools as [`crate::core::DynTool`]
//! implementations.

mod client;
mod tool;
mod toolset;

pub use client::{McpClient, McpStdioParams};
pub use tool::McpTool;
pub use toolset::McpToolset;
