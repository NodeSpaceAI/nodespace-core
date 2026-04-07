//! Shared parameter types for MCP and agent tool execution.
//!
//! These structs are used by both:
//! - MCP handlers for deserializing JSON-RPC request parameters
//! - Agent tool executor for unified argument parsing
//!
//! This eliminates duplicate argument parsing logic across both code paths.

pub use crate::mcp::handlers::nodes::{
    DeleteNodeParams, GetNodeParams, MCPCreateNodeParams, UpdateNodeParams,
};
pub use crate::mcp::handlers::search::{SearchNodesParams, SearchSemanticParams};
