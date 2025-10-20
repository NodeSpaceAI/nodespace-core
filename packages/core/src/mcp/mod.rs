//! Model Context Protocol (MCP) Integration
//!
//! Pure protocol implementation for stdio-based JSON-RPC 2.0 server.
//! Provides AI agent access to NodeSpace operations with no framework dependencies.
//!
//! # Architecture
//!
//! - **Pure business logic**: No Tauri dependencies in core protocol
//! - **Shared NodeService**: Wraps existing business logic services
//! - **stdio transport**: JSON-RPC 2.0 over stdin/stdout
//! - **Framework agnostic**: Can be integrated with any Rust application
//!
//! # Integration
//!
//! This module provides the protocol layer. Applications (like the Tauri desktop app)
//! wrap these handlers to add framework-specific functionality (e.g., event emission).
//!
//! # Usage
//!
//! AI agents send JSON-RPC requests via stdio:
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

pub use server::{run_mcp_server, run_mcp_server_with_callback, ResponseCallback};
pub use types::{MCPError, MCPRequest, MCPResponse};
