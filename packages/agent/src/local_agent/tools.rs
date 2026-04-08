//! Graph operation tools for the local agent.
//!
//! Implements [`AgentToolExecutor`] by wrapping `NodeService` and
//! `NodeEmbeddingService` methods as individual tools. Each tool validates its
//! arguments against a JSON schema, executes the corresponding service call, and
//! returns a compact, token-efficient result suitable for an 8k-context local model.

use crate::agent_types::{AgentToolExecutor, ToolDefinition, ToolError, ToolResult};
use async_trait::async_trait;
use nodespace_core::mcp::handlers::schema::handle_create_schema;
use nodespace_core::mcp::params::{SearchNodesParams, SearchSemanticParams};
use nodespace_core::ops::{node_ops, rel_ops, search_ops, OpsError};
use nodespace_core::services::{NodeEmbeddingService, NodeService};
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;

// ---------------------------------------------------------------------------
// Agent-specific parameter structs
//
// These complement the shared MCP params (re-exported via nodespace_core::mcp::params)
// for tools whose wire format differs from the MCP handler conventions
// (e.g., agent uses "title"+"body" while MCP uses "content").
// ---------------------------------------------------------------------------

/// Parameters for the agent's create_node tool (title+body model)
#[derive(Debug, Deserialize)]
struct AgentCreateNodeParams {
    pub title: String,
    pub node_type: String,
    #[serde(default)]
    pub body: Option<String>,
    #[serde(default)]
    pub parent_id: Option<String>,
    #[serde(default)]
    pub properties: Option<Value>,
}

/// Parameters for the agent's update_node tool (title+body model)
#[derive(Debug, Deserialize)]
struct AgentUpdateNodeParams {
    #[serde(alias = "node_id")]
    pub id: String,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub body: Option<String>,
    #[serde(default)]
    pub properties: Option<Value>,
}

/// Parameters for the agent's get_node tool (includes optional format field)
#[derive(Debug, Deserialize)]
struct AgentGetNodeParams {
    #[serde(alias = "node_id")]
    pub id: String,
    #[serde(default)]
    pub format: Option<String>,
}

/// Parameters for the create_relationship tool
#[derive(Debug, Deserialize)]
struct CreateRelationshipParams {
    pub from_id: String,
    pub to_id: String,
    pub relationship_type: String,
}

/// Parameters for the get_related_nodes tool
#[derive(Debug, Deserialize)]
struct GetRelatedNodesParams {
    #[serde(alias = "node_id")]
    pub id: String,
    #[serde(default)]
    pub relationship_type: Option<String>,
    #[serde(default)]
    pub direction: Option<String>,
}

/// Parameters for the find_skills tool
#[derive(Debug, Deserialize)]
struct FindSkillsParams {
    pub query: String,
    #[serde(default)]
    pub limit: Option<usize>,
}

/// Parameters for the update_task_status tool
#[derive(Debug, Deserialize)]
struct UpdateTaskStatusParams {
    #[serde(alias = "node_id")]
    pub id: String,
    pub status: String,
}

/// Parameters for the delete_node tool
#[derive(Debug, Deserialize)]
struct DeleteNodeParams {
    #[serde(alias = "node_id")]
    pub id: String,
}


/// Maximum characters for node body in full node results.
const BODY_TRUNCATE_FULL: usize = 2000;

/// Maximum characters for node body in list/summary results.
const BODY_TRUNCATE_SUMMARY: usize = 500;

/// Default search result limit.
const DEFAULT_SEARCH_LIMIT: usize = 10;

/// Default semantic search result limit.
const DEFAULT_SEMANTIC_LIMIT: usize = 5;

/// Minimum similarity threshold for semantic search.
const SEMANTIC_THRESHOLD: f32 = 0.3;

/// Truncate a string to `max_chars`, appending `[truncated]` if truncated.
fn truncate(s: &str, max_chars: usize) -> String {
    if s.len() <= max_chars {
        s.to_string()
    } else {
        // Find a safe char boundary
        let mut end = max_chars;
        while !s.is_char_boundary(end) && end > 0 {
            end -= 1;
        }
        format!("{}[truncated]", &s[..end])
    }
}

/// Build an error `ToolResult` from a string message.
fn error_result(tool_call_id: &str, name: &str, message: &str) -> ToolResult {
    ToolResult {
        tool_call_id: tool_call_id.to_string(),
        name: name.to_string(),
        result: json!({ "error": message }),
        is_error: true,
    }
}

/// Convert an OpsError to a ToolError.
fn ops_error_to_tool(e: OpsError, tool_name: &str) -> ToolError {
    ToolError::ExecutionFailed(format!("{} failed: {}", tool_name, e))
}

/// Build a success `ToolResult`.
fn ok_result(tool_call_id: &str, name: &str, data: Value) -> ToolResult {
    ToolResult {
        tool_call_id: tool_call_id.to_string(),
        name: name.to_string(),
        result: data,
        is_error: false,
    }
}

// ---------------------------------------------------------------------------
// Tool definitions (JSON schemas)
// ---------------------------------------------------------------------------

fn def_search_nodes() -> ToolDefinition {
    ToolDefinition {
        name: "search_nodes".into(),
        description: "Search for nodes by keyword or structured query".into(),
        parameters_schema: json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Keyword or phrase to search for in node content"
                },
                "node_type": {
                    "type": "string",
                    "description": "Optional filter by node type (text, task, date, etc.)"
                },
                "limit": {
                    "type": "integer",
                    "description": "Max results to return (default 10)"
                }
            },
            "required": ["query"]
        }),
    }
}

fn def_search_semantic() -> ToolDefinition {
    ToolDefinition {
        name: "search_semantic".into(),
        description: "Find nodes semantically related to a natural-language query. By default returns IDs and snippets only. Set include_markdown=1 to get full content for the top result, saving a separate get_node call.".into(),
        parameters_schema: json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Natural language query for semantic search"
                },
                "limit": {
                    "type": "integer",
                    "description": "Max results to return (default 5)"
                },
                "include_markdown": {
                    "type": "integer",
                    "description": "Number of top results to include full markdown content for (0-5, default 0). Set to 1 to get full content for the top result without a separate get_node call."
                },
                "collection": {
                    "type": "string",
                    "description": "Filter results to a specific collection path (e.g. 'Architecture', 'Development')"
                }
            },
            "required": ["query"]
        }),
    }
}

fn def_get_node() -> ToolDefinition {
    ToolDefinition {
        name: "get_node".into(),
        description: "Get a node by ID. Use format=markdown to include all descendants as a readable document.".into(),
        parameters_schema: json!({
            "type": "object",
            "properties": {
                "id": {
                    "type": "string",
                    "description": "Node ID to retrieve"
                },
                "format": {
                    "type": "string",
                    "enum": ["json", "markdown"],
                    "description": "Output format: json (default) returns node fields, markdown returns the node and all descendants as a readable document"
                }
            },
            "required": ["id"]
        }),
    }
}

fn def_create_node() -> ToolDefinition {
    ToolDefinition {
        name: "create_node".into(),
        description: "Create a new node with content".into(),
        parameters_schema: json!({
            "type": "object",
            "properties": {
                "title": {
                    "type": "string",
                    "description": "Title for the node"
                },
                "body": {
                    "type": "string",
                    "description": "Body/content text of the node"
                },
                "node_type": {
                    "type": "string",
                    "description": "Node type (text, task, etc.)"
                },
                "properties": {
                    "type": "object",
                    "description": "Additional properties as key-value pairs"
                },
                "parent_id": {
                    "type": "string",
                    "description": "Optional parent node ID"
                }
            },
            "required": ["title", "node_type"]
        }),
    }
}

fn def_update_node() -> ToolDefinition {
    ToolDefinition {
        name: "update_node".into(),
        description: "Update an existing node's content or properties".into(),
        parameters_schema: json!({
            "type": "object",
            "properties": {
                "id": {
                    "type": "string",
                    "description": "Node ID to update"
                },
                "title": {
                    "type": "string",
                    "description": "New title (optional)"
                },
                "body": {
                    "type": "string",
                    "description": "New body/content (optional)"
                },
                "properties": {
                    "type": "object",
                    "description": "Properties to merge/update (optional)"
                }
            },
            "required": ["id"]
        }),
    }
}

fn def_create_relationship() -> ToolDefinition {
    ToolDefinition {
        name: "create_relationship".into(),
        description: "Create a relationship between two nodes".into(),
        parameters_schema: json!({
            "type": "object",
            "properties": {
                "from_id": {
                    "type": "string",
                    "description": "Source node ID"
                },
                "to_id": {
                    "type": "string",
                    "description": "Target node ID"
                },
                "relationship_type": {
                    "type": "string",
                    "description": "Type of relationship (member_of, mentions, etc.)"
                }
            },
            "required": ["from_id", "to_id", "relationship_type"]
        }),
    }
}

fn def_get_related_nodes() -> ToolDefinition {
    ToolDefinition {
        name: "get_related_nodes".into(),
        description: "Get nodes related to a given node. Defaults to 'mentions' relationship type if not specified.".into(),
        parameters_schema: json!({
            "type": "object",
            "properties": {
                "id": {
                    "type": "string",
                    "description": "Node ID to find relations for"
                },
                "relationship_type": {
                    "type": "string",
                    "description": "Relationship type to query (default: 'mentions')"
                },
                "direction": {
                    "type": "string",
                    "enum": ["in", "out", "both"],
                    "description": "Direction of relationships (default: both)"
                }
            },
            "required": ["id"]
        }),
    }
}

fn def_find_skills() -> ToolDefinition {
    ToolDefinition {
        name: "find_skills".into(),
        description: "Search for agent skills by describing what you need to accomplish. Returns skill descriptions with available tools and guidance.".into(),
        parameters_schema: json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "What you need to accomplish"
                },
                "limit": {
                    "type": "integer",
                    "description": "Maximum skills to return (default 3)"
                }
            },
            "required": ["query"]
        }),
    }
}

fn def_create_schema() -> ToolDefinition {
    ToolDefinition {
        name: "create_schema".into(),
        description: "Create a new entity type (schema) with custom fields and relationships. The schema ID is auto-generated as lowercase kebab-case (e.g., 'Customer Profile' becomes 'customer-profile'). After creation, use this ID as node_type when creating instances. IMPORTANT: Do NOT include a 'name' or 'title' field — every node already has a content/title. EXCEPTION: if title_template uses a 'name' placeholder (e.g. '{name} ({status})'), you MUST define 'name' as a text field. Only define type-specific fields. If a field maps to an existing node type (e.g., 'tasks' maps to 'task'), define it as a relationship instead of an array field.".into(),
        parameters_schema: json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "Display name for the entity type (e.g., 'Project', 'Customer')"
                },
                "description": {
                    "type": "string",
                    "description": "Brief description of what this entity type represents"
                },
                "fields": {
                    "type": "array",
                    "description": "Array of field definitions. Only use for scalar properties (text, number, date, enum, boolean). Do NOT use for references to other node types — use relationships instead.",
                    "items": {
                        "type": "object",
                        "properties": {
                            "name": { "type": "string", "description": "Field name (e.g., 'status', 'email')" },
                            "type": { "type": "string", "description": "Field type: text, number, date, enum, array, object, boolean" },
                            "required": { "type": "boolean", "description": "Whether this field is required" },
                            "indexed": { "type": "boolean", "description": "Whether to index for search/filter" },
                            "description": { "type": "string", "description": "Field description" },
                            "coreValues": {
                                "type": "array",
                                "description": "For enum fields: array of {value, label} pairs. Use lowercase values (e.g., 'active' not 'Active').",
                                "items": {
                                    "type": "object",
                                    "properties": {
                                        "value": { "type": "string" },
                                        "label": { "type": "string" }
                                    }
                                }
                            }
                        },
                        "required": ["name", "type"]
                    }
                },
                "title_template": {
                    "type": "string",
                    "description": "Template for auto-generating node titles from field values. Use {field_name} placeholders, e.g. '{name} ({status})' or '{first_name} {last_name}'. IMPORTANT: every field referenced here MUST be defined in the fields array (e.g. if you use '{name}', add a 'name' text field). Only include fields that meaningfully identify the node. Omit if the content/title field alone is sufficient."
                },
                "relationships": {
                    "type": "array",
                    "description": "Relationships to other node types. Use instead of array fields when referencing existing types (e.g., project has_task task).",
                    "items": {
                        "type": "object",
                        "properties": {
                            "name": { "type": "string", "description": "Relationship name (e.g., 'has_task', 'assigned_to', 'depends_on')" },
                            "targetType": { "type": "string", "description": "Target node type ID — MUST be an existing type from the ENTITY TYPES list (e.g., 'task', 'project', 'customer'). Do NOT invent types that don't exist yet." },
                            "direction": { "type": "string", "enum": ["out", "in"], "description": "Direction: 'out' (this→target, default) or 'in' (target→this)" },
                            "cardinality": { "type": "string", "enum": ["one", "many"], "description": "Cardinality: 'one' or 'many' (default)" },
                            "description": { "type": "string", "description": "What this relationship represents" }
                        },
                        "required": ["name", "targetType", "direction", "cardinality"]
                    }
                }
            },
            "required": ["name"]
        }),
    }
}

fn def_delete_node() -> ToolDefinition {
    ToolDefinition {
        name: "delete_node".into(),
        description: "Delete a node from the knowledge graph by its ID. Use get_node first to confirm the node exists before deleting.".into(),
        parameters_schema: json!({
            "type": "object",
            "properties": {
                "id": {
                    "type": "string",
                    "description": "Node ID to delete"
                }
            },
            "required": ["id"]
        }),
    }
}

fn def_create_nodes_from_markdown() -> ToolDefinition {
    ToolDefinition {
        name: "create_nodes_from_markdown".into(),
        description: "Import a markdown document and create a hierarchy of nodes. Headings become parent nodes, content becomes child nodes.".into(),
        parameters_schema: json!({
            "type": "object",
            "properties": {
                "markdown": {
                    "type": "string",
                    "description": "Markdown content to import as nodes"
                },
                "parent_id": {
                    "type": "string",
                    "description": "Optional parent node ID to attach the import under"
                },
                "collection": {
                    "type": "string",
                    "description": "Optional collection path to add imported nodes to"
                }
            },
            "required": ["markdown"]
        }),
    }
}

fn def_update_task_status() -> ToolDefinition {
    ToolDefinition {
        name: "update_task_status".into(),
        description: "Update a task's status. Valid statuses: open, in_progress, done, cancelled."
            .into(),
        parameters_schema: json!({
            "type": "object",
            "properties": {
                "id": {
                    "type": "string",
                    "description": "Task node ID to update"
                },
                "status": {
                    "type": "string",
                    "enum": ["open", "in_progress", "done", "cancelled"],
                    "description": "New status value"
                }
            },
            "required": ["id", "status"]
        }),
    }
}

/// All tool definitions for the graph executor.
fn all_tool_definitions() -> Vec<ToolDefinition> {
    vec![
        def_search_nodes(),
        def_search_semantic(),
        def_get_node(),
        def_create_node(),
        def_update_node(),
        def_create_schema(),
        def_update_task_status(),
        def_create_relationship(),
        def_get_related_nodes(),
        def_find_skills(),
        def_delete_node(),
        def_create_nodes_from_markdown(),
    ]
}

// ---------------------------------------------------------------------------
// GraphToolExecutor
// ---------------------------------------------------------------------------

/// Executes graph operation tools against `NodeService` and `NodeEmbeddingService`.
///
/// Service references are injected directly, decoupling this crate from
/// Tauri-specific `AppServices`. The desktop-app layer is responsible for
/// resolving services and constructing this executor.
pub struct GraphToolExecutor {
    /// Node service for graph operations. `None` if services aren't initialized yet.
    pub node_service: Option<Arc<NodeService>>,
    /// Embedding service for semantic search. `None` if unavailable.
    pub embedding_service: Option<Arc<NodeEmbeddingService>>,
}

impl GraphToolExecutor {
    /// Create a new executor with the given services.
    pub fn new(
        node_service: Arc<NodeService>,
        embedding_service: Option<Arc<NodeEmbeddingService>>,
    ) -> Self {
        Self {
            node_service: Some(node_service),
            embedding_service,
        }
    }

    /// Create an executor with optional services.
    ///
    /// Use when services may not be initialized yet (e.g., at startup).
    /// Operations that need missing services will return a clear error.
    pub fn new_with_optional_services(
        node_service: Option<Arc<NodeService>>,
        embedding_service: Option<Arc<NodeEmbeddingService>>,
    ) -> Self {
        Self {
            node_service,
            embedding_service,
        }
    }

    // -- Individual tool implementations --

    async fn exec_search_nodes(
        &self,
        tool_call_id: &str,
        args: Value,
    ) -> Result<ToolResult, ToolError> {
        let params: SearchNodesParams =
            serde_json::from_value(args).map_err(|e| ToolError::InvalidArguments {
                tool: "search_nodes".to_string(),
                reason: e.to_string(),
            })?;
        let query = params.query;
        let node_type = params.node_type;
        let limit = params.limit.unwrap_or(DEFAULT_SEARCH_LIMIT);

        let ns = self.node_service()?;

        let input = node_ops::QueryNodesInput {
            node_type,
            parent_id: None,
            root_id: None,
            limit: Some(limit),
            offset: None,
            collection_id: None,
            collection: None,
            filters: Some(vec![node_ops::QueryFilterItem {
                field: "content".to_string(),
                operator: "contains".to_string(),
                value: Value::String(query),
            }]),
        };

        let output = node_ops::query_nodes(&ns, input)
            .await
            .map_err(|e| ops_error_to_tool(e, "search_nodes"))?;

        // Truncate node data for token efficiency
        let summaries: Vec<Value> = output
            .nodes
            .iter()
            .map(|v| {
                json!({
                    "id": v.get("id").and_then(|v| v.as_str()).unwrap_or(""),
                    "title": truncate(
                        v.get("title").and_then(|v| v.as_str()).unwrap_or(""),
                        100
                    ),
                    "type": v.get("node_type").or(v.get("type")).and_then(|v| v.as_str()).unwrap_or(""),
                    "snippet": truncate(
                        v.get("content").and_then(|v| v.as_str()).unwrap_or(""),
                        BODY_TRUNCATE_SUMMARY
                    ),
                })
            })
            .collect();

        Ok(ok_result(
            tool_call_id,
            "search_nodes",
            json!({ "count": summaries.len(), "nodes": summaries }),
        ))
    }

    async fn exec_search_semantic(
        &self,
        tool_call_id: &str,
        args: Value,
    ) -> Result<ToolResult, ToolError> {
        let params: SearchSemanticParams =
            serde_json::from_value(args).map_err(|e| ToolError::InvalidArguments {
                tool: "search_semantic".to_string(),
                reason: e.to_string(),
            })?;
        let query = params.query;
        let limit = params.limit.unwrap_or(DEFAULT_SEMANTIC_LIMIT);
        let include_markdown = params.include_markdown;
        let collection = params.collection;

        let ns = self.node_service()?;
        let emb = self.embedding_service()?;

        let input = search_ops::SearchSemanticInput {
            query: query.clone(),
            threshold: Some(SEMANTIC_THRESHOLD),
            limit: Some(limit),
            collection_id: None,
            collection,
            exclude_collections: None,
            include_markdown,
            include_archived: None,
            scope: None,
        };

        let output = search_ops::search_semantic(&ns, &emb, input)
            .await
            .map_err(|e| ops_error_to_tool(e, "search_semantic"))?;

        // Truncate for token efficiency
        let items: Vec<Value> = output
            .nodes
            .iter()
            .map(|v| {
                let mut item = json!({
                    "id": v.get("id").and_then(|v| v.as_str()).unwrap_or(""),
                    "title": truncate(
                        v.get("title").and_then(|v| v.as_str()).unwrap_or(""),
                        100
                    ),
                    "type": v.get("node_type").or(v.get("type")).and_then(|v| v.as_str()).unwrap_or(""),
                    "score": v.get("similarity").and_then(|v| v.as_f64()).unwrap_or(0.0),
                    "snippet": truncate(
                        v.get("content").and_then(|v| v.as_str()).unwrap_or(""),
                        BODY_TRUNCATE_SUMMARY
                    ),
                });
                // Include full markdown content if the ops layer returned it
                if let Some(md) = v.get("markdown").and_then(|v| v.as_str()) {
                    if !md.is_empty() {
                        item["markdown"] = json!(truncate(md, BODY_TRUNCATE_FULL));
                    }
                }
                item
            })
            .collect();

        Ok(ok_result(
            tool_call_id,
            "search_semantic",
            json!({ "count": items.len(), "results": items }),
        ))
    }

    async fn exec_get_node(
        &self,
        tool_call_id: &str,
        args: Value,
    ) -> Result<ToolResult, ToolError> {
        let params: AgentGetNodeParams =
            serde_json::from_value(args).map_err(|e| ToolError::InvalidArguments {
                tool: "get_node".to_string(),
                reason: e.to_string(),
            })?;
        let id = params.id;
        let format = params.format.unwrap_or_else(|| "json".to_string());

        let ns = self.node_service()?;

        if format == "markdown" {
            // Reuse the MCP handler's markdown export (single source of truth)
            use nodespace_core::mcp::handlers::markdown::handle_get_markdown_from_node_id;

            let params = json!({
                "node_id": id,
                "include_children": true,
                "include_node_ids": false,
            });
            match handle_get_markdown_from_node_id(&ns, params).await {
                Ok(result) => {
                    let md = result
                        .get("markdown")
                        .and_then(|t| t.as_str())
                        .unwrap_or("");
                    let truncated = truncate(md, BODY_TRUNCATE_FULL);
                    Ok(ok_result(
                        tool_call_id,
                        "get_node",
                        json!({ "markdown": truncated }),
                    ))
                }
                Err(e) => Ok(error_result(
                    tool_call_id,
                    "get_node",
                    &format!("Failed to render markdown: {:?}", e),
                )),
            }
        } else {
            let input = node_ops::GetNodeInput {
                node_id: id.clone(),
            };
            match node_ops::get_node(&ns, input).await {
                Ok(node_data) => Ok(ok_result(tool_call_id, "get_node", node_data)),
                Err(OpsError::NotFound { .. }) => Ok(error_result(
                    tool_call_id,
                    "get_node",
                    &format!("Node '{}' not found", id),
                )),
                Err(e) => Err(ops_error_to_tool(e, "get_node")),
            }
        }
    }

    async fn exec_create_node(
        &self,
        tool_call_id: &str,
        args: Value,
    ) -> Result<ToolResult, ToolError> {
        // Collect any flat (unknown) keys before consuming args into params.
        // This lets the model pass {"title": "X", "node_type": "task", "status": "open"}
        // and have "status" automatically promoted into properties.
        let flat_extras: serde_json::Map<String, Value> = {
            const KNOWN: &[&str] = &["title", "node_type", "body", "properties", "parent_id"];
            args.as_object()
                .map(|obj| {
                    obj.iter()
                        .filter(|(k, _)| !KNOWN.contains(&k.as_str()))
                        .map(|(k, v)| (k.clone(), v.clone()))
                        .collect()
                })
                .unwrap_or_default()
        };

        let params: AgentCreateNodeParams =
            serde_json::from_value(args).map_err(|e| ToolError::InvalidArguments {
                tool: "create_node".to_string(),
                reason: e.to_string(),
            })?;

        // Merge explicit properties with flat extras
        let mut props = params
            .properties
            .and_then(|v| v.as_object().cloned())
            .unwrap_or_default();
        props.extend(flat_extras);
        let properties = Value::Object(props);

        let ns = self.node_service()?;

        let body = params.body.unwrap_or_default();
        let content = if body.is_empty() {
            params.title.clone()
        } else {
            format!("{}\n{}", params.title, body)
        };

        let input = node_ops::CreateNodeInput {
            node_type: params.node_type,
            content,
            parent_id: params.parent_id,
            properties,
            collection: None,
            lifecycle_status: None,
        };

        let output = node_ops::create_node(&ns, input)
            .await
            .map_err(|e| ops_error_to_tool(e, "create_node"))?;

        Ok(ok_result(
            tool_call_id,
            "create_node",
            json!({ "id": output.node_id }),
        ))
    }

    async fn exec_update_node(
        &self,
        tool_call_id: &str,
        args: Value,
    ) -> Result<ToolResult, ToolError> {
        // Collect any flat (unknown) keys before consuming args into params.
        // This lets the model pass {"id": "...", "status": "done"} without
        // wrapping "status" in a "properties" object.
        let flat_extras: serde_json::Map<String, Value> = {
            const KNOWN: &[&str] = &["id", "node_id", "title", "body", "properties"];
            args.as_object()
                .map(|obj| {
                    obj.iter()
                        .filter(|(k, _)| !KNOWN.contains(&k.as_str()))
                        .map(|(k, v)| (k.clone(), v.clone()))
                        .collect()
                })
                .unwrap_or_default()
        };

        let params: AgentUpdateNodeParams =
            serde_json::from_value(args).map_err(|e| ToolError::InvalidArguments {
                tool: "update_node".to_string(),
                reason: e.to_string(),
            })?;

        // Merge explicit properties with flat extras
        let mut props = params
            .properties
            .and_then(|v| v.as_object().cloned())
            .unwrap_or_default();
        props.extend(flat_extras);
        let new_properties = if props.is_empty() {
            None
        } else {
            Some(Value::Object(props))
        };

        let content_update = match (&params.title, &params.body) {
            (Some(t), Some(b)) => Some(format!("{}\n{}", t, b)),
            (Some(t), None) => Some(t.clone()),
            (None, Some(b)) => Some(b.clone()),
            (None, None) => None,
        };

        if content_update.is_none() && new_properties.is_none() {
            return Err(ToolError::InvalidArguments {
                tool: "update_node".into(),
                reason: "At least one of 'title', 'body', or 'properties' must be provided".into(),
            });
        }

        let ns = self.node_service()?;

        let input = node_ops::UpdateNodeInput {
            node_id: params.id.clone(),
            version: None, // ops layer auto-fetches
            node_type: None,
            content: content_update,
            properties: new_properties,
            add_to_collection: None,
            remove_from_collection: None,
            lifecycle_status: None,
        };

        let output = node_ops::update_node(&ns, input)
            .await
            .map_err(|e| ops_error_to_tool(e, "update_node"))?;

        Ok(ok_result(
            tool_call_id,
            "update_node",
            json!({ "id": output.node_id, "updated": true }),
        ))
    }

    async fn exec_create_relationship(
        &self,
        tool_call_id: &str,
        args: Value,
    ) -> Result<ToolResult, ToolError> {
        let params: CreateRelationshipParams =
            serde_json::from_value(args).map_err(|e| ToolError::InvalidArguments {
                tool: "create_relationship".to_string(),
                reason: e.to_string(),
            })?;

        let ns = self.node_service()?;

        let input = rel_ops::CreateRelInput {
            source_id: params.from_id.clone(),
            relationship_name: params.relationship_type.clone(),
            target_id: params.to_id.clone(),
            edge_data: None,
        };

        rel_ops::create_relationship(&ns, input)
            .await
            .map_err(|e| ops_error_to_tool(e, "create_relationship"))?;

        Ok(ok_result(
            tool_call_id,
            "create_relationship",
            json!({ "from_id": params.from_id, "to_id": params.to_id, "type": params.relationship_type, "created": true }),
        ))
    }

    async fn exec_get_related_nodes(
        &self,
        tool_call_id: &str,
        args: Value,
    ) -> Result<ToolResult, ToolError> {
        let params: GetRelatedNodesParams =
            serde_json::from_value(args).map_err(|e| ToolError::InvalidArguments {
                tool: "get_related_nodes".to_string(),
                reason: e.to_string(),
            })?;
        let rel_type = params
            .relationship_type
            .unwrap_or_else(|| "mentions".to_string());
        let direction = params.direction.unwrap_or_else(|| "both".to_string());

        // Validate direction before acquiring the service
        let directions: Vec<&str> = match direction.as_str() {
            "out" => vec!["out"],
            "in" => vec!["in"],
            "both" => vec!["out", "in"],
            _ => {
                return Err(ToolError::InvalidArguments {
                    tool: "get_related_nodes".into(),
                    reason: "direction must be 'in', 'out', or 'both'".into(),
                });
            }
        };

        let ns = self.node_service()?;

        let mut all_nodes: Vec<Value> = Vec::new();
        for dir in &directions {
            let input = rel_ops::GetRelatedInput {
                node_id: params.id.clone(),
                relationship_name: rel_type.clone(),
                direction: dir.to_string(),
            };

            let output = rel_ops::get_related_nodes(&ns, input)
                .await
                .map_err(|e| ops_error_to_tool(e, "get_related_nodes"))?;

            for node_val in &output.related_nodes {
                let mut summary = json!({
                    "id": node_val.get("id").and_then(|v| v.as_str()).unwrap_or(""),
                    "title": truncate(
                        node_val.get("title").and_then(|v| v.as_str()).unwrap_or(""),
                        100
                    ),
                    "type": node_val.get("node_type").or(node_val.get("type")).and_then(|v| v.as_str()).unwrap_or(""),
                });
                summary["direction"] = json!(dir);
                summary["relationship_type"] = json!(&rel_type);
                all_nodes.push(summary);
            }
        }

        Ok(ok_result(
            tool_call_id,
            "get_related_nodes",
            json!({ "count": all_nodes.len(), "nodes": all_nodes }),
        ))
    }

    async fn exec_find_skills(
        &self,
        tool_call_id: &str,
        args: Value,
    ) -> Result<ToolResult, ToolError> {
        let params: FindSkillsParams =
            serde_json::from_value(args).map_err(|e| ToolError::InvalidArguments {
                tool: "find_skills".to_string(),
                reason: e.to_string(),
            })?;
        let limit = params.limit.unwrap_or(3);

        let emb = match &self.embedding_service {
            Some(svc) => svc,
            None => {
                return Ok(error_result(
                    tool_call_id,
                    "find_skills",
                    "Skill search unavailable: embedding service not loaded",
                ))
            }
        };

        use nodespace_core::ops::skill_ops;
        let output = skill_ops::find_skills(
            emb,
            skill_ops::FindSkillsInput {
                query: params.query.clone(),
                limit: Some(limit),
            },
        )
        .await
        .map_err(|e| ToolError::ExecutionFailed(format!("find_skills failed: {}", e)))?;

        if output.skills.is_empty() {
            Ok(ok_result(
                tool_call_id,
                "find_skills",
                json!({
                    "message": "No matching skills found. Proceed with general capabilities.",
                    "query": output.query
                }),
            ))
        } else {
            Ok(ok_result(
                tool_call_id,
                "find_skills",
                json!({
                    "count": output.skills.len(),
                    "skills": output.skills,
                    "query": output.query
                }),
            ))
        }
    }

    async fn exec_create_schema(
        &self,
        tool_call_id: &str,
        args: Value,
    ) -> Result<ToolResult, ToolError> {
        let ns = self.node_service()?;

        // Delegate to the MCP schema handler which handles ID normalization
        // (e.g., "Project" → "project"), field namespacing, and validation.
        let result = handle_create_schema(&ns, args)
            .await
            .map_err(|e| ToolError::ExecutionFailed(format!("create_schema failed: {:?}", e)))?;

        Ok(ok_result(tool_call_id, "create_schema", result))
    }

    async fn exec_update_task_status(
        &self,
        tool_call_id: &str,
        args: Value,
    ) -> Result<ToolResult, ToolError> {
        let params: UpdateTaskStatusParams =
            serde_json::from_value(args).map_err(|e| ToolError::InvalidArguments {
                tool: "update_task_status".to_string(),
                reason: e.to_string(),
            })?;

        // Validate status is a known enum value
        match params.status.as_str() {
            "open" | "in_progress" | "done" | "cancelled" => {}
            _ => {
                return Err(ToolError::InvalidArguments {
                    tool: "update_task_status".into(),
                    reason: format!(
                        "Invalid status '{}'. Must be one of: open, in_progress, done, cancelled",
                        params.status
                    ),
                });
            }
        }

        let ns = self.node_service()?;

        let input = node_ops::UpdateNodeInput {
            node_id: params.id.clone(),
            version: None,
            node_type: None,
            content: None,
            properties: Some(json!({ "status": params.status })),
            add_to_collection: None,
            remove_from_collection: None,
            lifecycle_status: None,
        };

        let output = node_ops::update_node(&ns, input)
            .await
            .map_err(|e| ops_error_to_tool(e, "update_task_status"))?;

        Ok(ok_result(
            tool_call_id,
            "update_task_status",
            json!({ "id": output.node_id, "status": params.status, "updated": true }),
        ))
    }

    async fn exec_delete_node(
        &self,
        tool_call_id: &str,
        args: Value,
    ) -> Result<ToolResult, ToolError> {
        let params: DeleteNodeParams =
            serde_json::from_value(args).map_err(|e| ToolError::InvalidArguments {
                tool: "delete_node".to_string(),
                reason: e.to_string(),
            })?;

        let ns = self.node_service()?;

        let input = node_ops::DeleteNodeInput {
            node_id: params.id.clone(),
            version: None, // ops layer auto-fetches
        };

        let output = node_ops::delete_node(&ns, input)
            .await
            .map_err(|e| ops_error_to_tool(e, "delete_node"))?;

        Ok(ok_result(
            tool_call_id,
            "delete_node",
            json!({ "node_id": output.node_id, "deleted": output.existed }),
        ))
    }

    async fn exec_create_nodes_from_markdown(
        &self,
        tool_call_id: &str,
        args: Value,
    ) -> Result<ToolResult, ToolError> {
        // Inline validation: require non-empty "markdown" field
        let markdown = args
            .get("markdown")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidArguments {
                tool: "create_nodes_from_markdown".to_string(),
                reason: "missing required field: markdown".to_string(),
            })?;
        if markdown.trim().is_empty() {
            return Err(ToolError::InvalidArguments {
                tool: "create_nodes_from_markdown".to_string(),
                reason: "markdown content must not be empty".to_string(),
            });
        }

        // Remap agent field names to MCP handler field names:
        // agent uses "markdown", handler expects "markdown_content"
        let mut handler_args = args.clone();
        if let Some(obj) = handler_args.as_object_mut() {
            if let Some(content) = obj.remove("markdown") {
                obj.insert("markdown_content".to_string(), content);
            }
        }

        let ns = self.node_service()?;

        // Delegate to the MCP markdown handler which handles the full import pipeline
        use nodespace_core::mcp::handlers::markdown::handle_create_nodes_from_markdown;
        let result = handle_create_nodes_from_markdown(&ns, handler_args)
            .await
            .map_err(|e| {
                ToolError::ExecutionFailed(format!("create_nodes_from_markdown failed: {:?}", e))
            })?;

        Ok(ok_result(
            tool_call_id,
            "create_nodes_from_markdown",
            result,
        ))
    }

    // -- Service accessors --

    fn node_service(&self) -> Result<Arc<NodeService>, ToolError> {
        self.node_service
            .clone()
            .ok_or_else(|| ToolError::ExecutionFailed("Node service unavailable".to_string()))
    }

    fn embedding_service(&self) -> Result<Arc<NodeEmbeddingService>, ToolError> {
        self.embedding_service
            .clone()
            .ok_or_else(|| ToolError::ExecutionFailed("Embedding service unavailable".to_string()))
    }
}

#[async_trait]
impl AgentToolExecutor for GraphToolExecutor {
    async fn available_tools(&self) -> Result<Vec<ToolDefinition>, ToolError> {
        Ok(all_tool_definitions())
    }

    async fn execute(&self, name: &str, args: Value) -> Result<ToolResult, ToolError> {
        // Use a synthetic tool_call_id derived from the tool name since the caller
        // (agent loop) will provide the real ID when it wraps the result.
        let tool_call_id = format!("call_{}", name);

        match name {
            "search_nodes" => self.exec_search_nodes(&tool_call_id, args).await,
            "search_semantic" => self.exec_search_semantic(&tool_call_id, args).await,
            "get_node" => self.exec_get_node(&tool_call_id, args).await,
            "create_node" => self.exec_create_node(&tool_call_id, args).await,
            "update_node" => self.exec_update_node(&tool_call_id, args).await,
            "create_schema" => self.exec_create_schema(&tool_call_id, args).await,
            "update_task_status" => self.exec_update_task_status(&tool_call_id, args).await,
            "create_relationship" => self.exec_create_relationship(&tool_call_id, args).await,
            "get_related_nodes" => self.exec_get_related_nodes(&tool_call_id, args).await,
            "find_skills" => self.exec_find_skills(&tool_call_id, args).await,
            "delete_node" => self.exec_delete_node(&tool_call_id, args).await,
            "create_nodes_from_markdown" => {
                self.exec_create_nodes_from_markdown(&tool_call_id, args)
                    .await
            }
            _ => Err(ToolError::UnknownTool(name.to_string())),
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Create a `GraphToolExecutor` with no backing services.
    ///
    /// Suitable for tests that validate argument parsing and tool dispatch
    /// without ever reaching a real database call.
    fn test_executor() -> GraphToolExecutor {
        GraphToolExecutor {
            node_service: None,
            embedding_service: None,
        }
    }

    // -- Helper: test truncation --

    #[test]
    fn truncate_short_string() {
        assert_eq!(truncate("hello", 10), "hello");
    }

    #[test]
    fn truncate_exact_boundary() {
        let s = "abcde";
        assert_eq!(truncate(s, 5), "abcde");
    }

    #[test]
    fn truncate_long_string() {
        let s = "a".repeat(600);
        let result = truncate(&s, BODY_TRUNCATE_SUMMARY);
        assert!(result.ends_with("[truncated]"));
        assert!(result.len() <= BODY_TRUNCATE_SUMMARY + "[truncated]".len());
    }

    #[test]
    fn truncate_multibyte() {
        // Ensure we don't split a multi-byte character
        let s = "Hello \u{1F600} world"; // emoji is 4 bytes
        let result = truncate(s, 8);
        assert!(result.ends_with("[truncated]"));
        // Should not panic
    }

    // -- Serde param parsing --

    #[test]
    fn search_nodes_params_parses_required_field() {
        let args = json!({ "query": "hello" });
        let params: SearchNodesParams = serde_json::from_value(args).unwrap();
        assert_eq!(params.query, "hello");
    }

    #[test]
    fn search_nodes_params_missing_query_fails() {
        let args = json!({});
        let result: Result<SearchNodesParams, _> = serde_json::from_value(args);
        assert!(result.is_err());
    }

    #[test]
    fn search_nodes_params_optional_limit() {
        let args = json!({ "query": "test", "limit": 20 });
        let params: SearchNodesParams = serde_json::from_value(args).unwrap();
        assert_eq!(params.limit, Some(20));

        let args_no_limit = json!({ "query": "test" });
        let params2: SearchNodesParams = serde_json::from_value(args_no_limit).unwrap();
        assert_eq!(params2.limit, None);
    }

    #[test]
    fn agent_get_node_params_accepts_id_alias() {
        let args = json!({ "id": "node-123" });
        let params: AgentGetNodeParams = serde_json::from_value(args).unwrap();
        assert_eq!(params.id, "node-123");
    }

    #[test]
    fn agent_update_node_params_accepts_id_alias() {
        let args = json!({ "id": "node-456", "title": "New title" });
        let params: AgentUpdateNodeParams = serde_json::from_value(args).unwrap();
        assert_eq!(params.id, "node-456");
        assert_eq!(params.title, Some("New title".to_string()));
    }

    // -- Tool definitions --

    #[test]
    fn all_definitions_have_unique_names() {
        let defs = all_tool_definitions();
        let names: Vec<&str> = defs.iter().map(|d| d.name.as_str()).collect();
        let unique: std::collections::HashSet<&str> = names.iter().copied().collect();
        assert_eq!(names.len(), unique.len(), "Duplicate tool names found");
    }

    #[test]
    fn definitions_count() {
        assert_eq!(all_tool_definitions().len(), 12);
    }

    #[test]
    fn each_definition_has_required_fields() {
        for def in all_tool_definitions() {
            assert!(!def.name.is_empty(), "Tool name must not be empty");
            assert!(
                !def.description.is_empty(),
                "Tool {} description must not be empty",
                def.name
            );
            assert!(
                def.parameters_schema.is_object(),
                "Tool {} schema must be an object",
                def.name
            );
            assert!(
                def.parameters_schema.get("type").is_some(),
                "Tool {} schema must have a type",
                def.name
            );
        }
    }

    #[test]
    fn search_nodes_schema_requires_query() {
        let def = def_search_nodes();
        let required = def.parameters_schema["required"]
            .as_array()
            .expect("required must be array");
        assert!(required.contains(&json!("query")));
    }

    #[test]
    fn create_node_schema_requires_title_and_type() {
        let def = def_create_node();
        let required = def.parameters_schema["required"]
            .as_array()
            .expect("required must be array");
        assert!(required.contains(&json!("title")));
        assert!(required.contains(&json!("node_type")));
    }

    #[test]
    fn create_relationship_schema_requires_all_three() {
        let def = def_create_relationship();
        let required = def.parameters_schema["required"]
            .as_array()
            .expect("required must be array");
        assert!(required.contains(&json!("from_id")));
        assert!(required.contains(&json!("to_id")));
        assert!(required.contains(&json!("relationship_type")));
    }

    // -- error_result / ok_result helpers --

    #[test]
    fn error_result_is_flagged() {
        let r = error_result("id1", "test", "something went wrong");
        assert!(r.is_error);
        assert_eq!(r.name, "test");
        assert_eq!(r.tool_call_id, "id1");
        assert!(r.result["error"]
            .as_str()
            .unwrap()
            .contains("something went wrong"));
    }

    #[test]
    fn ok_result_not_flagged() {
        let r = ok_result("id1", "test", json!({"key": "val"}));
        assert!(!r.is_error);
        assert_eq!(r.result["key"], "val");
    }

    // -- AgentToolExecutor trait: unknown tool --

    #[tokio::test]
    async fn execute_unknown_tool_returns_error() {
        let executor = test_executor();
        let result = executor.execute("nonexistent_tool", json!({})).await;
        assert!(result.is_err());
        match result.unwrap_err() {
            ToolError::UnknownTool(name) => assert_eq!(name, "nonexistent_tool"),
            other => panic!("Expected UnknownTool, got {:?}", other),
        }
    }

    // -- Validation: tools requiring arguments fail gracefully without services --

    #[tokio::test]
    async fn search_nodes_missing_query() {
        let executor = test_executor();
        let result = executor.execute("search_nodes", json!({})).await;
        assert!(result.is_err());
        match result.unwrap_err() {
            ToolError::InvalidArguments { tool, reason } => {
                assert_eq!(tool, "search_nodes");
                assert!(reason.contains("query"));
            }
            other => panic!("Expected InvalidArguments, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn get_node_missing_id() {
        let executor = test_executor();
        let result = executor.execute("get_node", json!({})).await;
        assert!(result.is_err());
        match result.unwrap_err() {
            ToolError::InvalidArguments { tool, .. } => {
                assert_eq!(tool, "get_node");
            }
            other => panic!("Expected InvalidArguments, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn create_node_missing_required() {
        let executor = test_executor();
        // Missing title
        let result = executor
            .execute("create_node", json!({"node_type": "text"}))
            .await;
        assert!(result.is_err());
        match result.unwrap_err() {
            ToolError::InvalidArguments { tool, reason } => {
                assert_eq!(tool, "create_node");
                assert!(reason.contains("title"));
            }
            other => panic!("Expected InvalidArguments, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn create_node_missing_type() {
        let executor = test_executor();
        let result = executor
            .execute("create_node", json!({"title": "Test"}))
            .await;
        assert!(result.is_err());
        match result.unwrap_err() {
            ToolError::InvalidArguments { tool, reason } => {
                assert_eq!(tool, "create_node");
                assert!(reason.contains("node_type"));
            }
            other => panic!("Expected InvalidArguments, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn update_node_missing_id() {
        let executor = test_executor();
        let result = executor
            .execute("update_node", json!({"title": "new"}))
            .await;
        assert!(result.is_err());
        match result.unwrap_err() {
            ToolError::InvalidArguments { tool, .. } => {
                assert_eq!(tool, "update_node");
            }
            other => panic!("Expected InvalidArguments, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn update_node_no_changes() {
        let executor = test_executor();
        let result = executor
            .execute("update_node", json!({"id": "node-1"}))
            .await;
        assert!(result.is_err());
        match result.unwrap_err() {
            ToolError::InvalidArguments { tool, reason } => {
                assert_eq!(tool, "update_node");
                assert!(reason.contains("At least one"));
            }
            other => panic!("Expected InvalidArguments, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn create_relationship_missing_fields() {
        let executor = test_executor();
        let result = executor
            .execute("create_relationship", json!({"from_id": "a"}))
            .await;
        assert!(result.is_err());
        match result.unwrap_err() {
            ToolError::InvalidArguments { tool, .. } => {
                assert_eq!(tool, "create_relationship");
            }
            other => panic!("Expected InvalidArguments, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn get_related_nodes_missing_id() {
        let executor = test_executor();
        let result = executor.execute("get_related_nodes", json!({})).await;
        assert!(result.is_err());
        match result.unwrap_err() {
            ToolError::InvalidArguments { tool, .. } => {
                assert_eq!(tool, "get_related_nodes");
            }
            other => panic!("Expected InvalidArguments, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn find_skills_missing_query() {
        let executor = test_executor();
        let result = executor.execute("find_skills", json!({})).await;
        assert!(result.is_err());
        match result.unwrap_err() {
            ToolError::InvalidArguments { tool, reason } => {
                assert_eq!(tool, "find_skills");
                assert!(reason.contains("query"));
            }
            other => panic!("Expected InvalidArguments, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn find_skills_no_embedding_service_returns_error_result() {
        let executor = test_executor();
        let result = executor
            .execute("find_skills", json!({"query": "manage tasks"}))
            .await;
        // Should succeed (Ok) but with is_error=true since no embedding service
        let tool_result = result.unwrap();
        assert!(tool_result.is_error);
        assert!(tool_result.result["error"]
            .as_str()
            .unwrap()
            .contains("embedding service"));
    }

    #[test]
    fn find_skills_schema_requires_query() {
        let def = def_find_skills();
        let required = def.parameters_schema["required"]
            .as_array()
            .expect("required must be array");
        assert!(required.contains(&json!("query")));
    }

    #[tokio::test]
    async fn get_related_nodes_invalid_direction() {
        let executor = test_executor();
        let result = executor
            .execute(
                "get_related_nodes",
                json!({"id": "node-1", "direction": "sideways"}),
            )
            .await;
        assert!(result.is_err());
        match result.unwrap_err() {
            ToolError::InvalidArguments { tool, reason } => {
                assert_eq!(tool, "get_related_nodes");
                assert!(reason.contains("direction"));
            }
            other => panic!("Expected InvalidArguments, got {:?}", other),
        }
    }

    // -- Available tools --

    #[tokio::test]
    async fn available_tools_returns_all() {
        let executor = test_executor();
        let tools = executor.available_tools().await.unwrap();
        assert_eq!(tools.len(), 12);
        let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
        assert!(names.contains(&"search_nodes"));
        assert!(names.contains(&"search_semantic"));
        assert!(names.contains(&"get_node"));
        assert!(names.contains(&"create_node"));
        assert!(names.contains(&"update_node"));
        assert!(names.contains(&"create_relationship"));
        assert!(names.contains(&"get_related_nodes"));
        assert!(names.contains(&"find_skills"));
        assert!(names.contains(&"create_schema"));
        assert!(names.contains(&"update_task_status"));
        assert!(names.contains(&"delete_node"));
        assert!(names.contains(&"create_nodes_from_markdown"));
    }
}
