//! Graph operation tools for the local agent.
//!
//! Implements [`AgentToolExecutor`] by wrapping `NodeService` and
//! `NodeEmbeddingService` methods as individual tools. Each tool validates its
//! arguments against a JSON schema, executes the corresponding service call, and
//! returns a compact, token-efficient result suitable for an 8k-context local model.

use crate::agent_types::{AgentToolExecutor, ToolDefinition, ToolError, ToolResult};
use async_trait::async_trait;
use nodespace_core::ops::{node_ops, rel_ops, search_ops, OpsError};
use nodespace_core::services::{NodeEmbeddingService, NodeService};
use serde_json::{json, Value};
use std::sync::Arc;

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

/// Extract a required string field from JSON args.
fn require_str(args: &Value, field: &str, tool_name: &str) -> Result<String, ToolError> {
    args.get(field)
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| ToolError::InvalidArguments {
            tool: tool_name.to_string(),
            reason: format!("'{}' is required and must be a string", field),
        })
}

/// Extract an optional string field from JSON args.
fn optional_str(args: &Value, field: &str) -> Option<String> {
    args.get(field)
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

/// Extract an optional integer field, with a default.
fn optional_usize(args: &Value, field: &str, default: usize) -> usize {
    args.get(field)
        .and_then(|v| v.as_u64())
        .map(|n| n as usize)
        .unwrap_or(default)
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
        description: "Find nodes semantically related to a natural-language query".into(),
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

/// All tool definitions for the graph executor.
fn all_tool_definitions() -> Vec<ToolDefinition> {
    vec![
        def_search_nodes(),
        def_search_semantic(),
        def_get_node(),
        def_create_node(),
        def_update_node(),
        def_create_relationship(),
        def_get_related_nodes(),
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
        let query = require_str(&args, "query", "search_nodes")?;
        let node_type = optional_str(&args, "node_type");
        let limit = optional_usize(&args, "limit", DEFAULT_SEARCH_LIMIT);

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
        let query = require_str(&args, "query", "search_semantic")?;
        let limit = optional_usize(&args, "limit", DEFAULT_SEMANTIC_LIMIT);

        let ns = self.node_service()?;
        let emb = self.embedding_service()?;

        let input = search_ops::SearchSemanticInput {
            query: query.clone(),
            threshold: Some(SEMANTIC_THRESHOLD),
            limit: Some(limit),
            collection_id: None,
            collection: None,
            exclude_collections: None,
            include_markdown: None,
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
                json!({
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
                })
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
        let id = require_str(&args, "id", "get_node")?;
        let format = optional_str(&args, "format").unwrap_or_else(|| "json".to_string());

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
                        .get("content")
                        .and_then(|c| c.as_array())
                        .and_then(|arr| arr.first())
                        .and_then(|item| item.get("text"))
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
            let input = node_ops::GetNodeInput { node_id: id.clone() };
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
        let title = require_str(&args, "title", "create_node")?;
        let node_type = require_str(&args, "node_type", "create_node")?;
        let body = optional_str(&args, "body").unwrap_or_default();
        let properties = args.get("properties").cloned().unwrap_or(json!({}));
        let parent_id = optional_str(&args, "parent_id");

        let ns = self.node_service()?;

        let content = if body.is_empty() {
            title.clone()
        } else {
            format!("{}\n{}", title, body)
        };

        let input = node_ops::CreateNodeInput {
            node_type,
            content,
            parent_id,
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
        let id = require_str(&args, "id", "update_node")?;
        let new_title = optional_str(&args, "title");
        let new_body = optional_str(&args, "body");
        let new_properties = args.get("properties").cloned();

        let content_update = match (&new_title, &new_body) {
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
            node_id: id.clone(),
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
        let from_id = require_str(&args, "from_id", "create_relationship")?;
        let to_id = require_str(&args, "to_id", "create_relationship")?;
        let rel_type = require_str(&args, "relationship_type", "create_relationship")?;

        let ns = self.node_service()?;

        let input = rel_ops::CreateRelInput {
            source_id: from_id.clone(),
            relationship_name: rel_type.clone(),
            target_id: to_id.clone(),
            edge_data: None,
        };

        rel_ops::create_relationship(&ns, input)
            .await
            .map_err(|e| ops_error_to_tool(e, "create_relationship"))?;

        Ok(ok_result(
            tool_call_id,
            "create_relationship",
            json!({ "from_id": from_id, "to_id": to_id, "type": rel_type, "created": true }),
        ))
    }

    async fn exec_get_related_nodes(
        &self,
        tool_call_id: &str,
        args: Value,
    ) -> Result<ToolResult, ToolError> {
        let id = require_str(&args, "id", "get_related_nodes")?;
        let rel_type =
            optional_str(&args, "relationship_type").unwrap_or_else(|| "mentions".to_string());
        let direction = optional_str(&args, "direction").unwrap_or_else(|| "both".to_string());

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
                node_id: id.clone(),
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
            "create_relationship" => self.exec_create_relationship(&tool_call_id, args).await,
            "get_related_nodes" => self.exec_get_related_nodes(&tool_call_id, args).await,
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

    // -- Helper: require_str validation --

    #[test]
    fn require_str_present() {
        let args = json!({ "query": "hello" });
        let result = require_str(&args, "query", "test_tool");
        assert_eq!(result.unwrap(), "hello");
    }

    #[test]
    fn require_str_missing() {
        let args = json!({});
        let result = require_str(&args, "query", "test_tool");
        assert!(result.is_err());
        match result.unwrap_err() {
            ToolError::InvalidArguments { tool, reason } => {
                assert_eq!(tool, "test_tool");
                assert!(reason.contains("query"));
            }
            _ => panic!("Expected InvalidArguments"),
        }
    }

    #[test]
    fn require_str_wrong_type() {
        let args = json!({ "query": 42 });
        let result = require_str(&args, "query", "test_tool");
        assert!(result.is_err());
    }

    // -- Helper: optional_usize --

    #[test]
    fn optional_usize_present() {
        let args = json!({ "limit": 20 });
        assert_eq!(optional_usize(&args, "limit", 10), 20);
    }

    #[test]
    fn optional_usize_missing_uses_default() {
        let args = json!({});
        assert_eq!(optional_usize(&args, "limit", 10), 10);
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
        assert_eq!(all_tool_definitions().len(), 7);
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
    async fn available_tools_returns_all_seven() {
        let executor = test_executor();
        let tools = executor.available_tools().await.unwrap();
        assert_eq!(tools.len(), 7);
        let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
        assert!(names.contains(&"search_nodes"));
        assert!(names.contains(&"search_semantic"));
        assert!(names.contains(&"get_node"));
        assert!(names.contains(&"create_node"));
        assert!(names.contains(&"update_node"));
        assert!(names.contains(&"create_relationship"));
        assert!(names.contains(&"get_related_nodes"));
    }
}
