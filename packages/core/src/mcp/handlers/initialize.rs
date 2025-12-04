//! MCP Initialize Handler
//!
//! Handles the MCP initialization handshake and capability discovery.
//! This is the first method called when a client connects to the server.
//!
//! The initialize handler dynamically builds instructions by querying available
//! node types from the database. This ensures instructions stay in sync with
//! the actual schema configuration and enables future per-user customization
//! of exposed tools and types.

use crate::mcp::types::MCPError;
use crate::services::NodeService;
use serde_json::{json, Value};
use std::sync::Arc;

/// Supported MCP protocol versions (for backward compatibility)
const SUPPORTED_PROTOCOL_VERSIONS: &[&str] = &[
    "2025-06-18", // Latest spec (future-proof)
    "2025-03-26", // Streamable HTTP (current)
    "2024-11-05", // HTTP+SSE (deprecated but supported)
];

/// Handle MCP initialize request
///
/// This is the FIRST method called when a client connects.
/// Returns server capabilities, protocol version, and dynamically-generated instructions.
///
/// # Protocol Flow
///
/// 1. Client sends initialize request with their protocol version
/// 2. Server validates version compatibility
/// 3. Server queries available node types from database
/// 4. Server builds instructions with current type list
/// 5. Server returns supported version + capabilities + instructions
/// 6. Client sends initialized notification (handled separately)
/// 7. Normal operations begin
///
/// # Arguments
///
/// * `node_service` - NodeService for querying available schemas
/// * `params` - Initialize request parameters containing protocolVersion and clientInfo
///
/// # Returns
///
/// Returns server capabilities including protocol version, server info, and dynamic instructions
///
/// # Errors
///
/// Returns error if:
/// - protocolVersion is missing or invalid
/// - Client requests unsupported protocol version
/// - Database query for schemas fails
pub async fn handle_initialize<C>(
    node_service: &Arc<NodeService<C>>,
    params: Value,
) -> Result<Value, MCPError>
where
    C: surrealdb::Connection,
{
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

    // Fetch all available schemas to build dynamic instructions
    // Note: If schemas haven't been initialized yet (fresh database), fall back to
    // built-in type list. This handles the bootstrap case where MCP server starts
    // before schema initialization completes.
    let schemas = node_service.get_all_schemas().await.unwrap_or_default();

    let node_types_str = if schemas.is_empty() {
        // Fallback: Built-in types (before schema initialization)
        "text (paragraphs), header (# headings), task (with status property), date (YYYY-MM-DD containers), code-block (```code```), quote-block (> quotes), ordered-list (1. items)".to_string()
    } else {
        // Dynamic: Build from actual schemas in database
        let node_types: Vec<String> = schemas
            .iter()
            .map(|schema| {
                let type_id = &schema.id; // Schema ID is the type name (e.g., "task", "text")
                // Add brief description for built-in types
                let desc = match type_id.as_str() {
                    "text" => "paragraphs",
                    "header" => "# headings",
                    "task" => "with status property",
                    "date" => "YYYY-MM-DD containers",
                    "code-block" => "```code```",
                    "quote-block" => "> quotes",
                    "ordered-list" => "1. items",
                    _ => "custom type", // User-defined schemas
                };
                format!("{} ({})", type_id, desc)
            })
            .collect();
        node_types.join(", ")
    };

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
        "instructions": format!(
            "NodeSpace is a knowledge management system where all content is stored as nodes in a hierarchical tree structure.\n\n\
            NODE ARCHITECTURE: Nodes are generic data structures with a type field. You can create custom node types via create_schema tool.\n\n\
            AVAILABLE NODE TYPES: {}. Use get_all_schemas to see full schema definitions with fields and relationships.\n\n\
            ROOT NODES (DOCUMENTS/PAGES/FILES): A root node represents a document, page, or file. It's a node with children but no parent (root_id = NULL). Two main patterns: (1) Date roots (YYYY-MM-DD format) for daily notes, (2) Topic roots for projects/notes. Use create_nodes_from_markdown to create complete documents with hierarchy.\n\n\
            DATE NODES: YYYY-MM-DD roots auto-exist. Just reference them as parent_id or root_id - no need to create them explicitly. Perfect for daily notes and time-based organization.\n\n\
            HIERARCHY CONTROL: Use insert_child_at_index and move_child_to_index for precise position control (0-based indexing). Use get_children for ordered child list, get_node_tree for full tree structure.\n\n\
            LINKING NODES: Use nodespace://node-id syntax to create bidirectional links. Example: 'See nodespace://abc-123' - automatically tracked in mentions/mentioned_by fields for graph navigation.\n\n\
            MARKDOWN WORKFLOWS: create_nodes_from_markdown imports documents preserving hierarchy. get_markdown_from_node_id exports with structure intact. update_root_from_markdown replaces entire document children (bulk edit).\n\n\
            SEARCH: search_roots uses vector embeddings for semantic search across all documents/pages.",
            node_types_str
        )
    }))
}

// Include tests
#[cfg(test)]
#[path = "initialize_test.rs"]
mod initialize_test;
