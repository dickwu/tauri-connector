//! Library surface of the standalone MCP server.
//!
//! The `tauri-connector-mcp` binary drives this over stdio; `connector-cli`
//! reuses [`tools::dispatch_tool`] so its `batch` command speaks the exact
//! same MCP tool vocabulary as both MCP servers.

pub mod protocol;
pub mod tools;
