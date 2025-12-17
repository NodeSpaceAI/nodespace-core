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

    // Version negotiation: Accept any valid MCP protocol version (YYYY-MM-DD format)
    // This future-proofs against Claude Code updates that use newer versions.
    // MCP spec: Server echoes back the client's version if supported.
    let is_valid_version = client_version.len() == 10
        && client_version.chars().nth(4) == Some('-')
        && client_version.chars().nth(7) == Some('-')
        && client_version[0..4].parse::<u32>().is_ok()
        && client_version[5..7].parse::<u32>().is_ok()
        && client_version[8..10].parse::<u32>().is_ok();

    if !is_valid_version {
        return Err(MCPError::invalid_request(format!(
            "Invalid protocol version format: {}. Expected YYYY-MM-DD format.",
            client_version
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
            PROGRESSIVE TOOL DISCOVERY: The tools/list response includes only 12 core tools (create_node, get_node, update_node, delete_node, query_nodes, get_children, insert_child_at_index, search_semantic, create_nodes_from_markdown, get_markdown_from_node_id, get_all_schemas, search_tools). Use search_tools to discover additional specialized tools by category, keyword, or node type. This reduces initial context and improves performance.\n\n\
            SEMANTIC SEARCH: Use search_semantic (core tool) to find relevant content across your knowledge base using natural language queries. Examples: 'Q4 planning documents', 'machine learning research notes'. This is a core value proposition of NodeSpace.\n\n\
            MARKDOWN WORKFLOWS: Use create_nodes_from_markdown (core tool) to import markdown documents as hierarchical nodes. Use get_markdown_from_node_id (core tool) to export nodes back to markdown format. These are primary workflows for NodeSpace.\n\n\
            NODE ARCHITECTURE: Nodes are generic data structures with a type field. You can create custom node types via create_schema tool (discoverable via search_tools).\n\n\
            AVAILABLE NODE TYPES: {}. Use get_all_schemas to see full schema definitions with fields and relationships.\n\n\
            ROOT NODES (DOCUMENTS/PAGES/FILES): A root node represents a document, page, or file. It's a node with children but no parent (root_id = NULL). Two main patterns: (1) Date roots (YYYY-MM-DD format) for daily notes, (2) Topic roots for projects/notes.\n\n\
            DATE NODES: YYYY-MM-DD roots auto-exist. Just reference them as parent_id or root_id - no need to create them explicitly. Perfect for daily notes and time-based organization.\n\n\
            HIERARCHY CONTROL: Core tools include insert_child_at_index and get_children. For advanced operations (move_child_to_index, get_node_tree), use search_tools with category='hierarchy'.\n\n\
            LINKING NODES: Use nodespace://node-id syntax to create bidirectional links. Example: 'See nodespace://abc-123' - automatically tracked in mentions/mentioned_by fields for graph navigation.\n\n\
            RELATIONSHIPS: For explicit typed relationships between nodes (beyond hierarchy), use search_tools with category='relationships' to discover create_relationship, get_related_nodes, and graph discovery tools.",
            node_types_str
        )
    }))
}

// Include tests
#[cfg(test)]
#[path = "initialize_test.rs"]
mod initialize_test;
