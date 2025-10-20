//! MCP Node CRUD Handlers
//!
//! Wraps NodeService operations for MCP protocol access.
//! Pure business logic - no Tauri dependencies.

use crate::mcp::types::MCPError;
use crate::models::{Node, NodeFilter, NodeUpdate, OrderBy};
use crate::services::NodeService;
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;

/// Parameters for create_node method
#[derive(Debug, Deserialize)]
pub struct CreateNodeParams {
    pub node_type: String,
    pub content: String,
    #[serde(default)]
    pub parent_id: Option<String>,
    #[serde(default)]
    pub container_node_id: Option<String>,
    #[serde(default)]
    pub before_sibling_id: Option<String>,
    #[serde(default)]
    pub properties: Value,
}

/// Parameters for get_node method
#[derive(Debug, Deserialize)]
pub struct GetNodeParams {
    pub node_id: String,
}

/// Parameters for update_node method
#[derive(Debug, Deserialize)]
pub struct UpdateNodeParams {
    pub node_id: String,
    #[serde(default)]
    pub node_type: Option<String>,
    #[serde(default)]
    pub content: Option<String>,
    #[serde(default)]
    pub properties: Option<Value>,
}

/// Parameters for delete_node method
#[derive(Debug, Deserialize)]
pub struct DeleteNodeParams {
    pub node_id: String,
}

/// Parameters for query_nodes method
#[derive(Debug, Deserialize)]
pub struct QueryNodesParams {
    #[serde(default)]
    pub node_type: Option<String>,
    #[serde(default)]
    pub parent_id: Option<String>,
    #[serde(default)]
    pub container_node_id: Option<String>,
    #[serde(default)]
    pub limit: Option<usize>,
    #[serde(default)]
    pub offset: Option<usize>,
}

/// Handle create_node MCP request
pub async fn handle_create_node(
    service: &Arc<NodeService>,
    params: Value,
) -> Result<Value, MCPError> {
    let params: CreateNodeParams = serde_json::from_value(params)
        .map_err(|e| MCPError::invalid_params(format!("Invalid parameters: {}", e)))?;

    // Create Node instance
    let node = Node::new(
        params.node_type.clone(),
        params.content,
        params.parent_id,
        params.properties,
    );

    // Set optional fields
    let node = Node {
        container_node_id: params.container_node_id,
        before_sibling_id: params.before_sibling_id,
        ..node
    };

    // Create node via NodeService
    let node_id = service
        .create_node(node)
        .await
        .map_err(|e| MCPError::node_creation_failed(format!("Failed to create node: {}", e)))?;

    Ok(json!({
        "node_id": node_id,
        "node_type": params.node_type,
        "success": true
    }))
}

/// Handle get_node MCP request
pub async fn handle_get_node(service: &Arc<NodeService>, params: Value) -> Result<Value, MCPError> {
    let params: GetNodeParams = serde_json::from_value(params)
        .map_err(|e| MCPError::invalid_params(format!("Invalid parameters: {}", e)))?;

    let node = service
        .get_node(&params.node_id)
        .await
        .map_err(|e| MCPError::internal_error(format!("Failed to get node: {}", e)))?;

    match node {
        Some(node) => {
            let value = serde_json::to_value(node).map_err(|e| {
                MCPError::internal_error(format!("Failed to serialize node: {}", e))
            })?;
            Ok(value)
        }
        None => Err(MCPError::node_not_found(&params.node_id)),
    }
}

/// Handle update_node MCP request
pub async fn handle_update_node(
    service: &Arc<NodeService>,
    params: Value,
) -> Result<Value, MCPError> {
    let params: UpdateNodeParams = serde_json::from_value(params)
        .map_err(|e| MCPError::invalid_params(format!("Invalid parameters: {}", e)))?;

    // Build NodeUpdate with intentional field restrictions
    //
    // MCP update_node intentionally restricts certain fields for data integrity:
    // - parent_id: Changing parent requires hierarchy validation and UI coordination
    // - container_node_id: Container changes affect multiple systems (embeddings, UI state)
    // - before_sibling_id: Ordering is managed by UI drag-drop interactions, not MCP
    // - embedding_vector: Embeddings are auto-generated from content via background jobs
    //
    // Use MCP only for content/property updates. Use Tauri commands for structural changes.
    let update = NodeUpdate {
        node_type: params.node_type,
        content: params.content,
        properties: params.properties,
        parent_id: None,
        container_node_id: None,
        before_sibling_id: None,
        embedding_vector: None,
    };

    // Update node via NodeService
    service
        .update_node(&params.node_id, update)
        .await
        .map_err(|e| MCPError::node_update_failed(format!("Failed to update node: {}", e)))?;

    Ok(json!({
        "node_id": params.node_id,
        "success": true
    }))
}

/// Handle delete_node MCP request
pub async fn handle_delete_node(
    service: &Arc<NodeService>,
    params: Value,
) -> Result<Value, MCPError> {
    let params: DeleteNodeParams = serde_json::from_value(params)
        .map_err(|e| MCPError::invalid_params(format!("Invalid parameters: {}", e)))?;

    // Delete node via NodeService
    service
        .delete_node(&params.node_id)
        .await
        .map_err(|e| MCPError::node_delete_failed(format!("Failed to delete node: {}", e)))?;

    Ok(json!({
        "node_id": params.node_id,
        "success": true
    }))
}

/// Handle query_nodes MCP request
pub async fn handle_query_nodes(
    service: &Arc<NodeService>,
    params: Value,
) -> Result<Value, MCPError> {
    let params: QueryNodesParams = serde_json::from_value(params)
        .map_err(|e| MCPError::invalid_params(format!("Invalid parameters: {}", e)))?;

    // Build NodeFilter using builder pattern
    let mut filter = NodeFilter::new();

    if let Some(node_type) = params.node_type {
        filter = filter.with_node_type(node_type);
    }

    if let Some(parent_id) = params.parent_id {
        filter = filter.with_parent_id(parent_id);
    }

    if let Some(container_node_id) = params.container_node_id {
        filter = filter.with_container_node_id(container_node_id);
    }

    if let Some(limit) = params.limit {
        filter = filter.with_limit(limit);
    }

    if let Some(offset) = params.offset {
        filter = filter.with_offset(offset);
    }

    filter = filter.with_order_by(OrderBy::CreatedDesc);

    // Query nodes via NodeService
    let nodes = service
        .query_nodes(filter)
        .await
        .map_err(|e| MCPError::internal_error(format!("Failed to query nodes: {}", e)))?;

    let value = serde_json::to_value(&nodes)
        .map_err(|e| MCPError::internal_error(format!("Failed to serialize nodes: {}", e)))?;

    Ok(json!({
        "nodes": value,
        "count": nodes.len()
    }))
}

// Include tests
#[cfg(test)]
#[path = "nodes_test.rs"]
mod nodes_test;
