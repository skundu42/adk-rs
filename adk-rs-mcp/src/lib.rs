//! MCP (Model Context Protocol) stdio client + [`McpToolset`].
//!
//! Spawns an MCP server as a child process, talks newline-delimited JSON-RPC
//! over stdin/stdout, and exposes discovered tools as [`adk_rs_core::DynTool`]
//! implementations.

#![forbid(unsafe_code)]
#![deny(missing_docs)]

mod client;
mod tool;
mod toolset;

pub use client::{McpClient, McpStdioParams};
pub use tool::McpTool;
pub use toolset::McpToolset;
