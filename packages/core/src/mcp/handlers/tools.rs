//! MCP Tools Handler
//!
//! Implements MCP-compliant tools/list and tools/call methods.
//! This module centralizes tool discovery and execution according to the
//! MCP 2024-11-05 specification.
//!
//! As of Issue #676, all handlers use NodeService directly instead of NodeOperations.
//! As of Issue #690, SchemaService was removed - schema nodes use generic CRUD.

use crate::mcp::handlers::{markdown, nodes, relationships, schema, search};
use crate::mcp::types::MCPError;
use crate::services::{NodeEmbeddingService, NodeService};
use serde_json::{json, Value};
use std::sync::Arc;

/// Handle tools/list MCP request
///
/// Returns all available tool schemas for client discovery.
/// This is called after initialize to discover what tools the server provides.
///
/// # MCP Spec Compliance
///
/// Response format:
/// ```json
/// {
///   "tools": [
///     {
///       "name": "tool_name",
///       "description": "...",
///       "inputSchema": { ... }
///     }
///   ]
/// }
/// ```
pub fn handle_tools_list(_params: Value) -> Result<Value, MCPError> {
    Ok(json!({
        "tools": get_tool_schemas()
    }))
}

/// Handle tools/call MCP request
///
/// Executes a tool by name with provided arguments.
/// This is the unified entry point for all tool execution in MCP-compliant servers.
///
/// # MCP Spec Compliance (2024-11-05)
///
/// Request format:
/// ```json
/// {
///   "name": "tool_name",
///   "arguments": { ... }
/// }
/// ```
///
/// Response format (success):
/// ```json
/// {
///   "content": [{
///     "type": "text",
///     "text": "..."
///   }],
///   "isError": false
/// }
/// ```
///
/// Response format (error):
/// ```json
/// {
///   "content": [{
///     "type": "text",
///     "text": "Error message"
///   }],
///   "isError": true
/// }
/// ```
///
/// # Arguments
///
/// * `node_service` - Arc reference to NodeService for node operations
/// * `embedding_service` - Arc reference to NodeEmbeddingService for search
/// * `params` - Request parameters containing `name` and `arguments`
///
/// # Returns
///
/// Returns JSON result with content array and isError flag per MCP spec
pub async fn handle_tools_call(
    node_service: &Arc<NodeService>,
    embedding_service: &Arc<NodeEmbeddingService>,
    params: Value,
) -> Result<Value, MCPError> {
    // Extract tool name from params
    let tool_name = params["name"]
        .as_str()
        .ok_or_else(|| MCPError::invalid_params("Missing 'name' parameter".to_string()))?;

    // Extract arguments (defaults to empty object if missing)
    let arguments = params.get("arguments").cloned().unwrap_or(json!({}));

    // Route to appropriate handler based on tool name
    let result = match tool_name {
        // Core Node CRUD
        "create_node" => nodes::handle_create_node(node_service, arguments).await,
        "get_node" => nodes::handle_get_node(node_service, arguments).await,
        "update_node" => nodes::handle_update_node(node_service, arguments).await,
        "delete_node" => nodes::handle_delete_node(node_service, arguments).await,
        "query_nodes" => nodes::handle_query_nodes(node_service, arguments).await,

        // Hierarchy & Children (Index-Based Operations)
        "get_children" => nodes::handle_get_children(node_service, arguments).await,
        "get_child_at_index" => nodes::handle_get_child_at_index(node_service, arguments).await,
        "insert_child_at_index" => {
            nodes::handle_insert_child_at_index(node_service, arguments).await
        }
        "move_child_to_index" => nodes::handle_move_child_to_index(node_service, arguments).await,
        "get_node_tree" => nodes::handle_get_node_tree(node_service, arguments).await,

        // Markdown Import/Export
        "create_nodes_from_markdown" => {
            markdown::handle_create_nodes_from_markdown(node_service, arguments).await
        }
        "get_markdown_from_node_id" => {
            markdown::handle_get_markdown_from_node_id(node_service, arguments).await
        }
        // New preferred tool name for root updates
        "update_root_from_markdown" => {
            markdown::handle_update_root_from_markdown(node_service, arguments).await
        }
        // DEPRECATED: Use update_root_from_markdown instead
        // Kept for backward compatibility - logs deprecation warning
        "update_container_from_markdown" => {
            tracing::warn!(
                "Tool 'update_container_from_markdown' is deprecated. Use 'update_root_from_markdown' instead."
            );
            markdown::handle_update_root_from_markdown(node_service, arguments).await
        }

        // Batch Operations
        "get_nodes_batch" => nodes::handle_get_nodes_batch(node_service, arguments).await,
        "update_nodes_batch" => nodes::handle_update_nodes_batch(node_service, arguments).await,

        // Search
        // New preferred tool name for root search
        "search_roots" => search::handle_search_roots(embedding_service, arguments),
        // DEPRECATED: Use search_roots instead
        // Kept for backward compatibility - logs deprecation warning
        "search_containers" => {
            tracing::warn!("Tool 'search_containers' is deprecated. Use 'search_roots' instead.");
            search::handle_search_roots(embedding_service, arguments)
        }

        // Schema creation (uses generic node creation)
        "create_schema" => schema::handle_create_schema(node_service, arguments).await,

        // Relationship CRUD (Issue #703)
        "create_relationship" => {
            relationships::handle_create_relationship(node_service, arguments).await
        }
        "delete_relationship" => {
            relationships::handle_delete_relationship(node_service, arguments).await
        }
        "get_related_nodes" => {
            relationships::handle_get_related_nodes(node_service, arguments).await
        }

        // NLP Discovery API (Issue #703)
        "get_relationship_graph" => {
            relationships::handle_get_relationship_graph(node_service, arguments).await
        }
        "get_inbound_relationships" => {
            relationships::handle_get_inbound_relationships(node_service, arguments).await
        }
        "get_all_schemas" => relationships::handle_get_all_schemas(node_service, arguments).await,

        _ => {
            return Err(MCPError::invalid_params(format!(
                "Unknown tool: {}",
                tool_name
            )))
        }
    };

    // Format response per MCP spec with content array and isError flag
    match result {
        Ok(data) => {
            // Success: Serialize result as pretty JSON text in content array
            let text = serde_json::to_string_pretty(&data).map_err(|e| {
                MCPError::internal_error(format!("JSON serialization failed: {}", e))
            })?;

            Ok(json!({
                "content": [{
                    "type": "text",
                    "text": text
                }],
                "isError": false
            }))
        }
        Err(e) => {
            // Error: Return error message in content array with isError=true
            // This follows MCP spec: tool execution errors are returned as successful
            // responses with isError=true, not as JSON-RPC errors
            Ok(json!({
                "content": [{
                    "type": "text",
                    "text": e.message
                }],
                "isError": true
            }))
        }
    }
}

/// Generate JSON schemas for all available MCP tools
///
/// This function defines the complete tool catalog exposed by the MCP server.
/// Schemas are manually maintained to provide high-quality descriptions and
/// precise control over the API surface.
///
/// # Design Rationale
///
/// Manual schemas (vs auto-generated) allow for:
/// - Human-crafted explanations optimized for AI understanding
/// - Detailed field-level documentation with examples
/// - Specific enum values that may differ from internal types
/// - Fine-grained control over what's exposed to MCP clients
///
/// # Future Enhancement
///
/// Consider auto-generating schemas from Rust types with proc macros,
/// while preserving ability to override descriptions (see Issue #312).
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
                    "root_id": {
                        "type": "string",
                        "description": "Optional root/document ID"
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
            "description": "Update an existing node's content or properties. Note: Core schema fields are protected - they cannot be deleted and enum values must match allowed values. User-defined fields can be freely modified.",
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
                        "description": "Updated properties (core fields are protected)"
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
            "name": "get_children",
            "description": "Get all children of a parent node in order with their positions (0-based indexes). Returns minimal info by default - use include_content=true to see node content.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "parent_id": {
                        "type": "string",
                        "description": "Parent node ID (any node ID, or YYYY-MM-DD for date containers)"
                    },
                    "include_content": {
                        "type": "boolean",
                        "description": "Include node content in response (default: false). Set to true only if you need content and don't already have it from get_markdown_from_node_id.",
                        "default": false
                    }
                },
                "required": ["parent_id"]
            }
        },
        {
            "name": "get_child_at_index",
            "description": "Get a specific child by its position under a parent. Returns the child node at the specified index (0-based).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "parent_id": {
                        "type": "string",
                        "description": "Parent node ID"
                    },
                    "index": {
                        "type": "number",
                        "description": "Position of child to retrieve (0-based)",
                        "minimum": 0
                    },
                    "include_content": {
                        "type": "boolean",
                        "description": "Include node content in response (default: true)",
                        "default": true
                    }
                },
                "required": ["parent_id", "index"]
            }
        },
        {
            "name": "insert_child_at_index",
            "description": "Insert a new child node at a specific position (0-based index) under a parent. Index 0 = first child, index 1 = second child, etc. If index >= child count, appends at end.\n\nDATE NODES: If parent_id is in YYYY-MM-DD format, it references a date container which auto-exists. You don't need to create date nodes first.\n\nExample: parent_id='2025-10-23' automatically uses that date's container.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "parent_id": {
                        "type": "string",
                        "description": "Parent node ID (any node ID, or YYYY-MM-DD for date containers)"
                    },
                    "index": {
                        "type": "number",
                        "description": "Position to insert at (0-based). 0=first, 1=second, etc. Use large number (e.g., 999) to append at end.",
                        "minimum": 0
                    },
                    "node_type": {
                        "type": "string",
                        "enum": ["text", "header", "task", "date", "code-block", "quote-block", "ordered-list"],
                        "description": "Type of node to create"
                    },
                    "content": {
                        "type": "string",
                        "description": "Node content"
                    },
                    "properties": {
                        "type": "object",
                        "description": "Additional type-specific properties (JSON object)"
                    }
                },
                "required": ["parent_id", "index", "node_type", "content"]
            }
        },
        {
            "name": "move_child_to_index",
            "description": "Move an existing child node to a different position among its siblings. The node stays under the same parent, only the position changes. Index 0 = first position, 1 = second, etc.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "node_id": {
                        "type": "string",
                        "description": "Child node to reorder"
                    },
                    "index": {
                        "type": "number",
                        "description": "New position (0-based). Node will be moved to this position among siblings. If index >= sibling count, moves to end.",
                        "minimum": 0
                    }
                },
                "required": ["node_id", "index"]
            }
        },
        {
            "name": "get_node_tree",
            "description": "Get hierarchical tree structure of a node and its descendants. Returns minimal structure by default (IDs, types, relationships). Use include_content=true if you need to see node content.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "node_id": {
                        "type": "string",
                        "description": "Root node ID to get tree from"
                    },
                    "max_depth": {
                        "type": "number",
                        "description": "Maximum depth to traverse (default: 10). Use lower values for performance with large trees.",
                        "default": 10,
                        "minimum": 1,
                        "maximum": 100
                    },
                    "include_content": {
                        "type": "boolean",
                        "description": "Include node content in response (default: false). Set to true only if you need content and don't have it from a previous get_markdown_from_node_id call.",
                        "default": false
                    },
                    "include_metadata": {
                        "type": "boolean",
                        "description": "Include created_at, modified_at, properties (default: false)",
                        "default": false
                    }
                },
                "required": ["node_id"]
            }
        },
        {
            "name": "create_nodes_from_markdown",
            "description": "Parse markdown and create hierarchical nodes. The container_title determines the container strategy: (1) Date format 'YYYY-MM-DD' creates/uses a date container, or (2) Markdown text (e.g., '# My Document' or 'Project Notes') creates a text/header container node. IMPORTANT: The container_title creates a separate container node, and all nodes from markdown_content become children of this container. Do NOT repeat the container_title in markdown_content to avoid duplicate nodes.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "markdown_content": {
                        "type": "string",
                        "description": "Markdown content to parse into nodes. These will become children of the container. Do NOT include the container_title text here to avoid duplication."
                    },
                    "container_title": {
                        "type": "string",
                        "description": "Container identifier (REQUIRED). Can be: (1) A date string in 'YYYY-MM-DD' format to use/create a date container, or (2) Markdown text (e.g., '# Project Alpha' or 'Meeting Notes') to create a text/header container. This creates a separate container node - do not repeat this text in markdown_content. The parsed container type must be text, header, or date - multi-line types (code-block, quote-block, ordered-list) cannot be containers."
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
        },
        {
            "name": "get_nodes_batch",
            "description": "Get multiple nodes in a single request (more efficient than multiple get_node calls). Useful when you need details for many nodes after parsing markdown export.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "node_ids": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Array of node IDs to retrieve (max 100)",
                        "maxItems": 100,
                        "minItems": 1
                    }
                },
                "required": ["node_ids"]
            }
        },
        {
            "name": "update_nodes_batch",
            "description": "Update multiple nodes in a single request (surgical updates). More efficient than calling update_node multiple times. Use this for bulk content updates like marking tasks complete.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "updates": {
                        "type": "array",
                        "description": "Array of update operations (max 100)",
                        "maxItems": 100,
                        "minItems": 1,
                        "items": {
                            "type": "object",
                            "properties": {
                                "id": { "type": "string", "description": "Node ID to update" },
                                "content": { "type": "string", "description": "Updated content" },
                                "node_type": { "type": "string", "description": "Updated node type" },
                                "properties": { "type": "object", "description": "Updated properties" }
                            },
                            "required": ["id"]
                        }
                    }
                },
                "required": ["updates"]
            }
        },
        {
            "name": "update_root_from_markdown",
            "description": "Replace all root node children with new structure parsed from markdown (bulk replacement, GitHub-style). Deletes existing children and creates new hierarchy. Use this when AI needs to reorganize or rewrite entire document structures.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "root_id": {
                        "type": "string",
                        "description": "Root node ID to update (also accepts 'container_id' for backward compatibility)"
                    },
                    "markdown": {
                        "type": "string",
                        "description": "New markdown content to parse and replace children. Will be parsed into nodes under the existing root."
                    }
                },
                "required": ["markdown"]
            }
        },
        {
            "name": "update_container_from_markdown",
            "description": "[DEPRECATED: Use update_root_from_markdown instead] Replace all container children with new structure parsed from markdown.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "container_id": {
                        "type": "string",
                        "description": "[DEPRECATED: Use root_id with update_root_from_markdown] Container node ID to update"
                    },
                    "markdown": {
                        "type": "string",
                        "description": "New markdown content to parse and replace children."
                    }
                },
                "required": ["container_id", "markdown"]
            }
        },
        {
            "name": "search_roots",
            "description": "Search root nodes using natural language semantic similarity (vector embeddings). Examples: 'Q4 planning tasks', 'machine learning research notes', 'budget discussions'",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "Natural language search query (e.g., 'Q4 planning tasks')"
                    },
                    "threshold": {
                        "type": "number",
                        "description": "Similarity threshold 0.0-1.0, lower = more similar (default: 0.7)",
                        "minimum": 0.0,
                        "maximum": 1.0,
                        "default": 0.7
                    },
                    "limit": {
                        "type": "number",
                        "description": "Maximum number of results (default: 20)",
                        "default": 20
                    },
                    "exact": {
                        "type": "boolean",
                        "description": "Use exact cosine distance instead of approximate DiskANN (default: false)",
                        "default": false
                    }
                },
                "required": ["query"]
            }
        },
        {
            "name": "search_containers",
            "description": "[DEPRECATED: Use search_roots instead] Search containers using natural language semantic similarity.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "Natural language search query"
                    },
                    "threshold": {
                        "type": "number",
                        "description": "Similarity threshold 0.0-1.0 (default: 0.7)",
                        "minimum": 0.0,
                        "maximum": 1.0,
                        "default": 0.7
                    },
                    "limit": {
                        "type": "number",
                        "description": "Maximum number of results (default: 20)",
                        "default": 20
                    },
                    "exact": {
                        "type": "boolean",
                        "description": "Use exact search (default: false)",
                        "default": false
                    }
                },
                "required": ["query"]
            }
        },
        // Schema creation tool
        {
            "name": "create_schema",
            "description": "Create a custom schema with fields and relationships. Fields can be provided explicitly or inferred from a natural language description. Relationships define edges to other node types.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "name": {
                        "type": "string",
                        "description": "Schema name (e.g., 'Invoice', 'Customer', 'Project')"
                    },
                    "description": {
                        "type": "string",
                        "description": "Optional natural language description of fields. Example: 'invoice number (required), amount in USD, status (draft/sent/paid)'. Used if 'fields' not provided."
                    },
                    "fields": {
                        "type": "array",
                        "description": "Optional explicit field definitions. Takes precedence over description parsing.",
                        "items": {
                            "type": "object",
                            "properties": {
                                "name": {"type": "string"},
                                "type": {"type": "string", "enum": ["string", "number", "boolean", "date", "enum", "array", "object"]},
                                "required": {"type": "boolean"},
                                "indexed": {"type": "boolean"},
                                "description": {"type": "string"}
                            },
                            "required": ["name", "type"]
                        }
                    },
                    "relationships": {
                        "type": "array",
                        "description": "Optional relationship definitions to other schemas",
                        "items": {
                            "type": "object",
                            "properties": {
                                "name": {"type": "string", "description": "Relationship name (e.g., 'billed_to', 'assigned_to')"},
                                "targetType": {"type": "string", "description": "Target schema ID (e.g., 'customer', 'person')"},
                                "direction": {"type": "string", "enum": ["out", "in"], "default": "out"},
                                "cardinality": {"type": "string", "enum": ["one", "many"], "default": "one"},
                                "required": {"type": "boolean", "default": false},
                                "reverseName": {"type": "string", "description": "Optional name for reverse lookups (e.g., 'invoices')"},
                                "reverseCardinality": {"type": "string", "enum": ["one", "many"]},
                                "edgeFields": {
                                    "type": "array",
                                    "description": "Optional fields stored on the edge",
                                    "items": {
                                        "type": "object",
                                        "properties": {
                                            "name": {"type": "string"},
                                            "type": {"type": "string"},
                                            "required": {"type": "boolean"}
                                        }
                                    }
                                }
                            },
                            "required": ["name", "targetType"]
                        }
                    },
                    "additional_constraints": {
                        "type": "object",
                        "description": "Optional constraints for description parsing (only used when description provided)",
                        "properties": {
                            "required_fields": {
                                "type": "array",
                                "items": {"type": "string"},
                                "description": "List of field names that are required"
                            },
                            "enum_values": {
                                "type": "object",
                                "description": "Map of field names to their enum values"
                            }
                        }
                    }
                },
                "required": ["name"]
            }
        },
        // Relationship CRUD tools (Issue #703)
        {
            "name": "create_relationship",
            "description": "Create a relationship between two nodes. The relationship must be defined in the source node's schema. Edge data can include field values defined in the relationship's edgeFields.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "source_id": {
                        "type": "string",
                        "description": "ID of the source node"
                    },
                    "relationship_name": {
                        "type": "string",
                        "description": "Name of the relationship (must be defined in source node's schema)"
                    },
                    "target_id": {
                        "type": "string",
                        "description": "ID of the target node"
                    },
                    "edge_data": {
                        "type": "object",
                        "description": "Optional edge field values (JSON object)"
                    }
                },
                "required": ["source_id", "relationship_name", "target_id"]
            }
        },
        {
            "name": "delete_relationship",
            "description": "Delete a relationship between two nodes. This is idempotent - succeeds even if the edge doesn't exist.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "source_id": {
                        "type": "string",
                        "description": "ID of the source node"
                    },
                    "relationship_name": {
                        "type": "string",
                        "description": "Name of the relationship"
                    },
                    "target_id": {
                        "type": "string",
                        "description": "ID of the target node"
                    }
                },
                "required": ["source_id", "relationship_name", "target_id"]
            }
        },
        {
            "name": "get_related_nodes",
            "description": "Get all nodes connected via a specific relationship. Supports both forward ('out') and reverse ('in') directions.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "node_id": {
                        "type": "string",
                        "description": "ID of the node to get relationships for"
                    },
                    "relationship_name": {
                        "type": "string",
                        "description": "Name of the relationship"
                    },
                    "direction": {
                        "type": "string",
                        "enum": ["out", "in"],
                        "description": "Direction to traverse: 'out' for forward, 'in' for reverse (default: 'out')"
                    }
                },
                "required": ["node_id", "relationship_name"]
            }
        },
        // NLP Discovery tools (Issue #703)
        {
            "name": "get_relationship_graph",
            "description": "Get a summary of all relationships defined in schemas. Returns the complete relationship graph for understanding the data model structure.",
            "inputSchema": {
                "type": "object",
                "properties": {},
                "required": []
            }
        },
        {
            "name": "get_inbound_relationships",
            "description": "Discover all relationships from other schemas that point TO a specific node type. Useful for understanding reverse relationships without mutating target schemas.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "target_type": {
                        "type": "string",
                        "description": "The node type to find inbound relationships for (e.g., 'customer', 'person')"
                    }
                },
                "required": ["target_type"]
            }
        },
        {
            "name": "get_all_schemas",
            "description": "Get all schema definitions including their fields and relationships. This is the primary entry point for understanding the complete data model.",
            "inputSchema": {
                "type": "object",
                "properties": {},
                "required": []
            }
        }
    ])
}

// Include tests
#[cfg(test)]
#[path = "tools_test.rs"]
mod tools_test;
