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
        "instructions": "NodeSpace is a knowledge management system where all content is stored as nodes in a hierarchical tree structure.\n\nCONTAINERS AND DOCUMENTS: Containers = Documents. A container is a node that has children (via container_node_id). There are two types: (1) Date containers (YYYY-MM-DD format) for daily notes, (2) Topic containers for projects and organized content. Use create_nodes_from_markdown to create full documents.\n\nNODE TYPES: text, header, task, date, code-block, quote-block, ordered-list. Each represents a content chunk within a document.\n\nLINKING NODES: Use nodespace://node-id to create links. Example: 'See nodespace://abc-123' - automatically tracked in mentions/mentioned_by fields.\n\nDATE NODES: YYYY-MM-DD containers auto-exist. Just reference as parent_id or container_node_id - no need to create explicitly.\n\nHIERARCHY: Use insert_child_at_index, move_child_to_index for position control (0-based). Use get_children for ordered list, get_node_tree for full structure.\n\nMARKDOWN: create_nodes_from_markdown imports documents, get_markdown_from_node_id exports with hierarchy preserved.\n\nSEARCH: search_containers uses vector embeddings for semantic search across all documents."
    }))
}

// Include tests
#[cfg(test)]
#[path = "initialize_test.rs"]
mod initialize_test;
