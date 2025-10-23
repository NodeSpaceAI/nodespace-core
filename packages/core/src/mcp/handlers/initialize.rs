//! MCP Initialize Handler
//!
//! Handles the MCP initialization handshake and capability discovery.
//! This is the first method called when a client connects to the server.

use crate::mcp::types::MCPError;
use serde_json::{json, Value};

/// Supported MCP protocol versions (for backward compatibility)
const SUPPORTED_PROTOCOL_VERSIONS: &[&str] = &[
    "2025-06-18", // Latest spec (future-proof)
    "2025-03-26", // Streamable HTTP (current)
    "2024-11-05", // HTTP+SSE (deprecated but supported)
];

/// Handle MCP initialize request
///
/// This is the FIRST method called when a client connects.
/// Returns server capabilities, protocol version, and tool schemas.
///
/// # Protocol Flow
///
/// 1. Client sends initialize request with their protocol version
/// 2. Server validates version compatibility
/// 3. Server returns supported version + capabilities
/// 4. Client sends initialized notification (handled separately)
/// 5. Normal operations begin
///
/// # Arguments
///
/// * `params` - Initialize request parameters containing protocolVersion and clientInfo
///
/// # Returns
///
/// Returns server capabilities including protocol version, server info, and available tools
///
/// # Errors
///
/// Returns error if:
/// - protocolVersion is missing or invalid
/// - Client requests unsupported protocol version
pub fn handle_initialize(params: Value) -> Result<Value, MCPError> {
    // Parse client's initialize request
    let client_version = params["protocolVersion"]
        .as_str()
        .ok_or_else(|| MCPError::invalid_params("Missing protocolVersion parameter".to_string()))?;

    // Version negotiation: Check if we support client's version
    // MCP spec: Server should respond with same version if supported,
    // or suggest alternative version
    if !SUPPORTED_PROTOCOL_VERSIONS.contains(&client_version) {
        return Err(MCPError::invalid_request(format!(
            "Unsupported protocol version: {}. Server supports: {:?}",
            client_version, SUPPORTED_PROTOCOL_VERSIONS
        )));
    }

    // Build capability response per MCP spec
    // Tools capability indicates support for tools/list and tools/call
    // Actual tool schemas are retrieved via tools/list method
    Ok(json!({
        "protocolVersion": client_version,  // Echo back client's version if supported
        "serverInfo": {
            "name": "nodespace-mcp-server",
            "version": env!("CARGO_PKG_VERSION")
        },
        "capabilities": {
            "tools": {
                "listChanged": false  // Tool list is static, doesn't change after init
            },
            "resources": {},  // Future: Add resource capabilities
            "prompts": {}     // Future: Add prompt capabilities
        },
        "instructions": {
            "date_nodes": "Date nodes (YYYY-MM-DD) are virtual containers that auto-exist for any valid date. You don't need to create them explicitly - just reference them as parent_id or container_node_id and they'll be created automatically if needed. Example: insert_child_at_index(parent_id='2025-10-23', ...) automatically creates the date container.",
            "node_types": "Available node types: text, header, task, date, code-block, quote-block, ordered-list. Date nodes are special containers for daily notes.",
            "hierarchy": "Nodes can be nested. Use index-based operations (insert_child_at_index, move_child_to_index) for intuitive position control, or use low-level operations (create_node, reorder_node) for direct pointer manipulation."
        }
    }))
}

// Include tests
#[cfg(test)]
#[path = "initialize_test.rs"]
mod initialize_test;
