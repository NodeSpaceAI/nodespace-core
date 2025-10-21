//! MCP Initialize Handler
//!
//! Handles the MCP initialization handshake and capability discovery.
//! This is the first method called when a client connects to the server.

use crate::mcp::types::MCPError;
use serde_json::{json, Value};

/// MCP protocol version supported by this server
const MCP_PROTOCOL_VERSION: &str = "2024-11-05";

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
    if client_version != MCP_PROTOCOL_VERSION {
        // For now, only support latest version
        // Future: Could negotiate older versions
        return Err(MCPError::invalid_request(format!(
            "Unsupported protocol version: {}. Server supports: {}",
            client_version, MCP_PROTOCOL_VERSION
        )));
    }

    // Build capability response
    Ok(json!({
        "protocolVersion": MCP_PROTOCOL_VERSION,
        "serverInfo": {
            "name": "nodespace-mcp-server",
            "version": env!("CARGO_PKG_VERSION")
        },
        "capabilities": {
            "tools": get_tool_schemas(),
            "resources": {},  // Future: Add resource capabilities
            "prompts": {}     // Future: Add prompt capabilities
        }
    }))
}

/// Generate JSON schemas for all available MCP tools
///
/// TODO: Consider auto-generating from Rust types (future enhancement)
/// For now, manually maintain schemas to ensure quality descriptions.
/// Manual schemas allow for:
/// - Human-crafted explanations and descriptions
/// - Detailed field-level documentation
/// - Specific enum values that may not be in types
/// - Fine-grained control over what's exposed to clients
fn get_tool_schemas() -> Value {
    json!([
        {
            "name": "create_node",
            "description": "Create a new node in NodeSpace",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "node_type": {
                        "type": "string",
                        "enum": ["text", "header", "task", "date", "code-block", "quote-block", "ordered-list"],
                        "description": "Type of node to create"
                    },
                    "content": {
                        "type": "string",
                        "description": "Content of the node (markdown format for most types)"
                    },
                    "parent_id": {
                        "type": "string",
                        "description": "Optional parent node ID for hierarchy"
                    },
                    "container_node_id": {
                        "type": "string",
                        "description": "Optional container/document ID"
                    },
                    "properties": {
                        "type": "object",
                        "description": "Additional type-specific properties (JSON object)"
                    }
                },
                "required": ["node_type", "content"]
            }
        },
        {
            "name": "get_node",
            "description": "Retrieve a single node by ID",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "node_id": {
                        "type": "string",
                        "description": "ID of the node to retrieve"
                    }
                },
                "required": ["node_id"]
            }
        },
        {
            "name": "update_node",
            "description": "Update an existing node's content or properties",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "node_id": {
                        "type": "string",
                        "description": "ID of the node to update"
                    },
                    "content": {
                        "type": "string",
                        "description": "Updated content"
                    },
                    "properties": {
                        "type": "object",
                        "description": "Updated properties"
                    }
                },
                "required": ["node_id"]
            }
        },
        {
            "name": "delete_node",
            "description": "Delete a node and optionally its children",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "node_id": {
                        "type": "string",
                        "description": "ID of the node to delete"
                    }
                },
                "required": ["node_id"]
            }
        },
        {
            "name": "query_nodes",
            "description": "Query nodes with filters",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "filters": {
                        "type": "array",
                        "description": "Array of filter conditions"
                    },
                    "limit": {
                        "type": "number",
                        "description": "Maximum number of results"
                    }
                }
            }
        },
        {
            "name": "create_nodes_from_markdown",
            "description": "Parse markdown and create hierarchical nodes",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "markdown_content": {
                        "type": "string",
                        "description": "Markdown content to parse"
                    },
                    "container_title": {
                        "type": "string",
                        "description": "Title for the container node"
                    }
                },
                "required": ["markdown_content", "container_title"]
            }
        },
        {
            "name": "get_markdown_from_node_id",
            "description": "Export node and its children as clean markdown for reading and analysis",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "node_id": {
                        "type": "string",
                        "description": "Root node ID to export"
                    },
                    "include_children": {
                        "type": "boolean",
                        "description": "Include child nodes recursively (default: true)",
                        "default": true
                    },
                    "max_depth": {
                        "type": "number",
                        "description": "Maximum recursion depth (default: 20)",
                        "default": 20
                    }
                },
                "required": ["node_id"]
            }
        }
    ])
}

// Include tests
#[cfg(test)]
#[path = "initialize_test.rs"]
mod initialize_test;
