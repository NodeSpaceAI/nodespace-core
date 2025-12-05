//! MCP Tools Handler
//!
//! Implements MCP-compliant tools/list and tools/call methods.
//! This module centralizes tool discovery and execution according to the
//! MCP 2024-11-05 specification.
//!
//! ## Progressive Disclosure
//!
//! Following Anthropic's advanced tool use patterns, tools are organized into tiers:
//! - **Tier 1 (Core)**: Always exposed in tools/list (~450-900 tokens)
//! - **Tier 2 (Discoverable)**: Found via search_tools (~1,700 tokens saved initially)
//!
//! All tools remain callable via tools/call regardless of tier.
//!
//! As of Issue #676, all handlers use NodeService directly instead of NodeOperations.
//! As of Issue #690, SchemaService was removed - schema nodes use generic CRUD.

use crate::mcp::handlers::{markdown, nodes, relationships, schema, search};
use crate::mcp::types::MCPError;
use crate::services::{NodeEmbeddingService, NodeService};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::Arc;

/// Tool exposure tier for progressive disclosure
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolTier {
    /// Core tools always exposed in tools/list (Tier 1)
    Core,
    /// Discoverable tools found via search_tools (Tier 2)
    Discoverable,
}

/// Tool category for filtering and organization
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ToolCategory {
    /// Basic CRUD operations (create_node, get_node, update_node, delete_node)
    Crud,
    /// Query and batch operations (query_nodes, get_nodes_batch, update_nodes_batch)
    Query,
    /// Hierarchy operations (get_children, insert_child_at_index, etc.)
    Hierarchy,
    /// Markdown import/export (create_nodes_from_markdown, etc.)
    Markdown,
    /// Semantic search operations (search_semantic)
    Search,
    /// Schema management (create_schema, get_all_schemas, update_schema)
    Schema,
    /// Relationship operations (create_relationship, get_related_nodes, etc.)
    Relationships,
    /// Discovery and meta-tools (search_tools)
    Discovery,
}

/// Parameters for search_tools
#[derive(Debug, Deserialize)]
pub struct SearchToolsParams {
    /// Search query (searches names and descriptions)
    #[serde(default)]
    pub query: Option<String>,

    /// Filter by category
    #[serde(default)]
    pub category: Option<ToolCategory>,

    /// Filter by node type (returns type-specific tools)
    #[serde(default)]
    pub node_type: Option<String>,

    /// Maximum results to return
    #[serde(default = "default_search_limit")]
    pub limit: usize,
}

fn default_search_limit() -> usize {
    10
}

/// Get the tier for a given tool
///
/// FUTURE: This will be user-configurable, allowing users to customize which tools
/// are exposed as "core" vs "discoverable" based on their workflows and preferences.
/// For now, we hardcode based on common use cases (CRUD, markdown, search).
fn get_tool_tier(tool_name: &str) -> ToolTier {
    match tool_name {
        // Tier 1: Core CRUD operations
        "create_node" | "get_node" | "update_node" | "delete_node" => ToolTier::Core,

        // Tier 1: Essential query
        "query_nodes" => ToolTier::Core,

        // Tier 1: Basic hierarchy
        "get_children" | "insert_child_at_index" => ToolTier::Core,

        // Tier 1: Semantic search (core value proposition)
        "search_semantic" => ToolTier::Core,

        // Tier 1: Markdown import/export (primary workflows)
        "create_nodes_from_markdown" | "get_markdown_from_node_id" => ToolTier::Core,

        // Tier 1: Discovery
        "get_all_schemas" | "search_tools" => ToolTier::Core,

        // Tier 2: Everything else is discoverable
        _ => ToolTier::Discoverable,
    }
}

/// Get the category for a given tool
fn get_tool_category(tool_name: &str) -> ToolCategory {
    match tool_name {
        "create_node" | "get_node" | "update_node" | "delete_node" => ToolCategory::Crud,

        "query_nodes" | "get_nodes_batch" | "update_nodes_batch" => ToolCategory::Query,

        "get_children"
        | "insert_child_at_index"
        | "move_child_to_index"
        | "get_child_at_index"
        | "get_node_tree" => ToolCategory::Hierarchy,

        "create_nodes_from_markdown"
        | "get_markdown_from_node_id"
        | "update_root_from_markdown" => ToolCategory::Markdown,

        "search_semantic" => ToolCategory::Search,

        "create_schema" | "get_all_schemas" | "update_schema" => ToolCategory::Schema,

        "create_relationship"
        | "delete_relationship"
        | "get_related_nodes"
        | "get_relationship_graph"
        | "get_inbound_relationships"
        | "add_schema_relationship"
        | "remove_schema_relationship" => ToolCategory::Relationships,

        "search_tools" => ToolCategory::Discovery,

        _ => ToolCategory::Query, // Default fallback
    }
}

/// Check if a tool supports a specific node type
/// Currently returns true for generic tools, false for type-specific tools that don't match
fn tool_supports_node_type(_tool_name: &str, _node_type: &str) -> bool {
    // Future: When we have type-specific tools like set_task_status
    // match (tool_name, node_type) {
    //     ("set_task_status", "task") => true,
    //     ("get_tasks_by_status", "task") => true,
    //     _ => false
    // }

    // For now, all tools support all types (generic operations)
    true
}

/// Handle search_tools request
///
/// Progressive discovery tool that allows AI agents to find tools dynamically
/// instead of loading all tool definitions upfront.
///
/// # Parameters
///
/// - `query`: Optional text search across tool names and descriptions
/// - `category`: Optional filter by tool category (crud, query, hierarchy, etc.)
/// - `node_type`: Optional filter for type-specific tools
/// - `limit`: Maximum number of results (default: 10)
///
/// # Returns
///
/// Returns filtered tool definitions matching the search criteria
pub fn handle_search_tools(params: Value) -> Result<Value, MCPError> {
    let params: SearchToolsParams = serde_json::from_value(params)
        .map_err(|e| MCPError::invalid_params(format!("Invalid parameters: {}", e)))?;

    // Get all tool schemas
    let all_tools = get_tool_schemas();
    let tools_array = all_tools
        .as_array()
        .ok_or_else(|| MCPError::internal_error("Tool schemas not an array".to_string()))?;

    // Apply filters (only return Tier 2 Discoverable tools)
    let filtered: Vec<Value> = tools_array
        .iter()
        .filter(|tool| {
            let name = tool["name"].as_str().unwrap_or("");
            let desc = tool["description"].as_str().unwrap_or("");
            let category = get_tool_category(name);

            // Only include Tier 2 (Discoverable) tools
            // Tier 1 tools are already exposed via tools/list
            if matches!(get_tool_tier(name), ToolTier::Core) {
                return false;
            }

            // Query filter (case-insensitive search in name and description)
            if let Some(q) = &params.query {
                let query_lower = q.to_lowercase();
                if !name.to_lowercase().contains(&query_lower)
                    && !desc.to_lowercase().contains(&query_lower)
                {
                    return false;
                }
            }

            // Category filter
            if let Some(cat) = &params.category {
                if category != *cat {
                    return false;
                }
            }

            // Node type filter
            if let Some(node_type) = &params.node_type {
                if !tool_supports_node_type(name, node_type) {
                    return false;
                }
            }

            true
        })
        .take(params.limit)
        .cloned()
        .collect();

    Ok(json!({
        "tools": filtered,
        "total": filtered.len(),
        "query": params.query,
        "category": params.category
    }))
}

/// Handle tools/list MCP request
///
/// Returns Tier 1 (core) tool schemas for progressive disclosure.
/// This is called after initialize to discover what tools the server provides.
///
/// ## Progressive Disclosure
///
/// Only Tier 1 tools are exposed initially (~450-900 tokens).
/// AI agents can discover Tier 2 tools via search_tools (~1,700 token savings).
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
    // Get all tool schemas
    let all_tools = get_tool_schemas();
    let tools_array = all_tools
        .as_array()
        .ok_or_else(|| MCPError::internal_error("Tool schemas not an array".to_string()))?;

    // Filter to only Tier 1 (Core) tools
    let tier1_tools: Vec<Value> = tools_array
        .iter()
        .filter(|tool| {
            let name = tool["name"].as_str().unwrap_or("");
            matches!(get_tool_tier(name), ToolTier::Core)
        })
        .cloned()
        .collect();

    Ok(json!({
        "tools": tier1_tools
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
pub async fn handle_tools_call<C>(
    node_service: &Arc<NodeService<C>>,
    embedding_service: &Arc<NodeEmbeddingService<C>>,
    params: Value,
) -> Result<Value, MCPError>
where
    C: surrealdb::Connection,
{
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
        // Root node bulk replacement
        "update_root_from_markdown" => {
            markdown::handle_update_root_from_markdown(node_service, arguments).await
        }

        // Batch Operations
        "get_nodes_batch" => nodes::handle_get_nodes_batch(node_service, arguments).await,
        "update_nodes_batch" => nodes::handle_update_nodes_batch(node_service, arguments).await,

        // Search
        "search_semantic" => search::handle_search_semantic(embedding_service, arguments).await,

        // Discovery
        "search_tools" => handle_search_tools(arguments),

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

        // Schema Definition Management (Issue #703)
        "add_schema_relationship" => {
            schema::handle_add_schema_relationship(node_service, arguments).await
        }
        "remove_schema_relationship" => {
            schema::handle_remove_schema_relationship(node_service, arguments).await
        }
        "update_schema" => schema::handle_update_schema(node_service, arguments).await,

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
            "description": "Parse markdown and create hierarchical nodes. If 'title' is provided, it becomes the root node and markdown_content becomes children. If 'title' is omitted, the first line of markdown_content is used as the root node title.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "markdown_content": {
                        "type": "string",
                        "description": "Markdown content to parse into nodes. If title is provided, this becomes children of the title node. If title is omitted, the first line becomes the root and the rest become children."
                    },
                    "title": {
                        "type": "string",
                        "description": "Optional root node title. Can be: (1) A date string in 'YYYY-MM-DD' format to use/create a date root, or (2) Markdown text (e.g., '# Project Alpha' or 'Meeting Notes') to create a text/header root. If omitted, the first line of markdown_content is used as the root."
                    }
                },
                "required": ["markdown_content"]
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
                    },
                    "include_node_ids": {
                        "type": "boolean",
                        "description": "Include node ID comments in markdown output (default: true). When true, adds HTML comments with node IDs and versions for OCC. When false, produces clean markdown without metadata.",
                        "default": true
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
            "description": "Replace all children of a root node (document/page/file) with new structure parsed from markdown (bulk replacement, GitHub-style). Deletes all existing children and creates new hierarchy. Use this when AI needs to reorganize or rewrite entire document structures. Note: The root node itself is preserved - only its children are replaced.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "root_id": {
                        "type": "string",
                        "description": "Root node ID to update (synonymous with document/page/file ID)"
                    },
                    "markdown": {
                        "type": "string",
                        "description": "New markdown content to parse and replace children. Will be parsed into nodes under the existing root."
                    }
                },
                "required": ["root_id", "markdown"]
            }
        },
        {
            "name": "search_semantic",
            "description": "Search root nodes by semantic similarity using vector embeddings. Returns only root nodes (documents/pages) - use get_markdown_from_node_id with the returned node ID to retrieve the full document content. Examples: 'Q4 planning documents', 'machine learning research notes'",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "Natural language search query (e.g., 'Q4 planning tasks')"
                    },
                    "threshold": {
                        "type": "number",
                        "description": "Minimum similarity threshold 0.0-1.0. Results must have similarity > this value. Higher = stricter (default: 0.7)",
                        "minimum": 0.0,
                        "maximum": 1.0,
                        "default": 0.7
                    },
                    "limit": {
                        "type": "number",
                        "description": "Maximum number of results (default: 20)",
                        "default": 20
                    }
                },
                "required": ["query"]
            }
        },
        // Progressive disclosure - tool discovery
        {
            "name": "search_tools",
            "description": "Discover additional NodeSpace tools by category, node type, or keyword search. Use this to find specialized tools beyond the core set (create_node, get_node, update_node, delete_node, query_nodes, get_children, insert_child_at_index, get_all_schemas).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "Optional search query to match against tool names and descriptions (e.g., 'markdown', 'relationship', 'batch')"
                    },
                    "category": {
                        "type": "string",
                        "description": "Optional category filter",
                        "enum": ["crud", "query", "hierarchy", "markdown", "search", "schema", "relationships", "discovery"]
                    },
                    "node_type": {
                        "type": "string",
                        "description": "Optional filter for tools relevant to a specific node type (e.g., 'task', 'text', 'date')"
                    },
                    "limit": {
                        "type": "number",
                        "description": "Maximum number of tools to return (default: 10)",
                        "default": 10,
                        "minimum": 1,
                        "maximum": 50
                    }
                }
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
        },
        // Schema Definition Management (Issue #703)
        {
            "name": "add_schema_relationship",
            "description": "Add a relationship definition to an existing schema. This creates the edge table DDL and enables relationship CRUD operations between nodes of this schema and the target type.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "schema_id": {
                        "type": "string",
                        "description": "ID of the schema to add the relationship to"
                    },
                    "relationship": {
                        "type": "object",
                        "description": "Relationship definition",
                        "properties": {
                            "name": {
                                "type": "string",
                                "description": "Name of the relationship (used in API calls, e.g., 'billed_to', 'assigned_to')"
                            },
                            "target_type": {
                                "type": "string",
                                "description": "Target node type (e.g., 'customer', 'person')"
                            },
                            "cardinality": {
                                "type": "string",
                                "enum": ["one", "many"],
                                "description": "Cardinality from source perspective (default: 'many')"
                            },
                            "reverse_name": {
                                "type": "string",
                                "description": "Optional reverse name for NLP discovery (e.g., 'invoices' when viewed from customer)"
                            },
                            "reverse_cardinality": {
                                "type": "string",
                                "enum": ["one", "many"],
                                "description": "Cardinality from target perspective (for NLP understanding)"
                            },
                            "description": {
                                "type": "string",
                                "description": "Human-readable description of the relationship"
                            },
                            "edge_fields": {
                                "type": "array",
                                "items": {
                                    "type": "object",
                                    "properties": {
                                        "name": {"type": "string"},
                                        "field_type": {"type": "string", "enum": ["string", "number", "boolean", "datetime"]},
                                        "required": {"type": "boolean"},
                                        "indexed": {"type": "boolean"}
                                    },
                                    "required": ["name", "field_type"]
                                },
                                "description": "Optional fields stored on the edge (e.g., 'role', 'since')"
                            }
                        },
                        "required": ["name", "target_type"]
                    }
                },
                "required": ["schema_id", "relationship"]
            }
        },
        {
            "name": "remove_schema_relationship",
            "description": "Remove a relationship definition from a schema. This is a soft-delete: the edge table and existing data are preserved, but the relationship is hidden from the schema definition. Use this to deprecate relationships without losing historical data.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "schema_id": {
                        "type": "string",
                        "description": "ID of the schema to remove the relationship from"
                    },
                    "relationship_name": {
                        "type": "string",
                        "description": "Name of the relationship to remove"
                    }
                },
                "required": ["schema_id", "relationship_name"]
            }
        },
        {
            "name": "update_schema",
            "description": "Update a schema's mutable properties in a single operation. Supports updating description, adding/removing fields, and adding/removing relationships. For bulk changes, this is more efficient than individual operations.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "schema_id": {
                        "type": "string",
                        "description": "ID of the schema to update"
                    },
                    "description": {
                        "type": "string",
                        "description": "New description for the schema"
                    },
                    "add_fields": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "name": {"type": "string"},
                                "type": {"type": "string", "enum": ["string", "number", "boolean", "datetime", "enum", "array", "object"]},
                                "required": {"type": "boolean"},
                                "indexed": {"type": "boolean"},
                                "description": {"type": "string"}
                            },
                            "required": ["name", "type"]
                        },
                        "description": "Fields to add to the schema"
                    },
                    "remove_fields": {
                        "type": "array",
                        "items": {"type": "string"},
                        "description": "Names of fields to remove"
                    },
                    "add_relationships": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "name": {"type": "string"},
                                "target_type": {"type": "string"},
                                "cardinality": {"type": "string", "enum": ["one", "many"]},
                                "reverse_name": {"type": "string"},
                                "reverse_cardinality": {"type": "string", "enum": ["one", "many"]},
                                "description": {"type": "string"}
                            },
                            "required": ["name", "target_type"]
                        },
                        "description": "Relationships to add to the schema"
                    },
                    "remove_relationships": {
                        "type": "array",
                        "items": {"type": "string"},
                        "description": "Names of relationships to remove (soft-delete)"
                    }
                },
                "required": ["schema_id"]
            }
        }
    ])
}

// Include tests
#[cfg(test)]
#[path = "tools_test.rs"]
mod tools_test;
