//! Model Context Protocol (MCP) Integration
//!
//! Provides stdio-based JSON-RPC 2.0 server for AI agent access to NodeSpace.
//! Runs as an async Tokio task within the Tauri application process.
//!
//! # Architecture
//!
//! - Single-process: MCP runs as background task, not separate process
//! - Shared NodeService: Same instance used by Tauri commands
//! - Tauri events: Operations emit events for UI reactivity
//! - stdio transport: JSON-RPC over stdin/stdout for AI agent communication
//!
//! # Usage
//!
//! AI agents spawn NodeSpace with stdio piping and send JSON-RPC requests:
//!
//! ```json
//! {
//!   "jsonrpc": "2.0",
//!   "id": 1,
//!   "method": "create_node",
//!   "params": {
//!     "node_type": "task",
//!     "content": "Review quarterly reports"
//!   }
//! }
//! ```
//!
//! See `/docs/architecture/business-logic/mcp-integration.md` for full details.

pub mod handlers;
pub mod server;
pub mod types;

pub use server::run_mcp_server;
pub use types::{MCPError, MCPRequest, MCPResponse};
