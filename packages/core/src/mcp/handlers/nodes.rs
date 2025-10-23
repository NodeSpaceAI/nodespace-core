//! MCP Node CRUD Handlers
//!
//! Wraps NodeOperations for MCP protocol access.
//! Pure business logic - no Tauri dependencies.

use crate::mcp::types::MCPError;
use crate::models::{NodeFilter, OrderBy};
use crate::operations::NodeOperations;
use chrono::NaiveDate;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashMap;
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

/// Parameters for get_children method
#[derive(Debug, Deserialize)]
pub struct GetChildrenParams {
    pub parent_id: String,
    #[serde(default)]
    pub include_content: bool,
}

/// Parameters for insert_child_at_index method
#[derive(Debug, Deserialize)]
pub struct InsertChildAtIndexParams {
    pub parent_id: String,
    pub index: usize,
    pub node_type: String,
    pub content: String,
    #[serde(default)]
    pub properties: Value,
}

/// Parameters for move_child_to_index method
#[derive(Debug, Deserialize)]
pub struct MoveChildToIndexParams {
    pub node_id: String,
    pub index: usize,
}

/// Parameters for get_child_at_index method
#[derive(Debug, Deserialize)]
pub struct GetChildAtIndexParams {
    pub parent_id: String,
    pub index: usize,
    #[serde(default = "default_true")]
    pub include_content: bool,
}

/// Parameters for get_node_tree method
#[derive(Debug, Deserialize)]
pub struct GetNodeTreeParams {
    pub node_id: String,
    #[serde(default = "default_max_depth")]
    pub max_depth: usize,
    #[serde(default)]
    pub include_content: bool,
    #[serde(default)]
    pub include_metadata: bool,
}

fn default_true() -> bool {
    true
}

fn default_max_depth() -> usize {
    10
}

/// Child information for ordered list
#[derive(Debug, serde::Serialize)]
pub struct ChildInfo {
    pub index: usize,
    pub node_id: String,
    pub node_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
}

/// Tree node information for hierarchical display
#[derive(Debug, serde::Serialize)]
pub struct TreeNode {
    pub node_id: String,
    pub node_type: String,
    pub depth: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub before_sibling_id: Option<String>,
    pub child_count: usize,
    pub children: Vec<TreeNode>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub modified_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub properties: Option<Value>,
}

/// Handle create_node MCP request
pub async fn handle_create_node(
    operations: &Arc<NodeOperations>,
    params: Value,
) -> Result<Value, MCPError> {
    let params: CreateNodeParams = serde_json::from_value(params)
        .map_err(|e| MCPError::invalid_params(format!("Invalid parameters: {}", e)))?;

    // Create node via NodeOperations (enforces all business rules)
    let node_id = operations
        .create_node(
            params.node_type.clone(),
            params.content,
            params.parent_id,
            params.container_node_id,
            params.before_sibling_id,
            params.properties,
        )
        .await
        .map_err(|e| MCPError::node_creation_failed(format!("Failed to create node: {}", e)))?;

    Ok(json!({
        "node_id": node_id,
        "node_type": params.node_type,
        "success": true
    }))
}

/// Handle get_node MCP request
pub async fn handle_get_node(
    operations: &Arc<NodeOperations>,
    params: Value,
) -> Result<Value, MCPError> {
    let params: GetNodeParams = serde_json::from_value(params)
        .map_err(|e| MCPError::invalid_params(format!("Invalid parameters: {}", e)))?;

    let node = operations
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
    operations: &Arc<NodeOperations>,
    params: Value,
) -> Result<Value, MCPError> {
    let params: UpdateNodeParams = serde_json::from_value(params)
        .map_err(|e| MCPError::invalid_params(format!("Invalid parameters: {}", e)))?;

    // Update node via NodeOperations (enforces Rule 5: content updates only, no hierarchy changes)
    //
    // MCP update_node intentionally restricts certain fields for data integrity:
    // - parent_id: Use move_node operation for parent changes
    // - container_node_id: Auto-calculated with parent changes
    // - before_sibling_id: Use reorder_node operation for sibling changes
    // - embedding_vector: Embeddings are auto-generated from content via background jobs
    //
    // Use MCP only for content/property updates. Use separate operations for structural changes.
    operations
        .update_node(
            &params.node_id,
            params.content,
            params.node_type,
            params.properties,
        )
        .await
        .map_err(|e| MCPError::node_update_failed(format!("Failed to update node: {}", e)))?;

    Ok(json!({
        "node_id": params.node_id,
        "success": true
    }))
}

/// Handle delete_node MCP request
pub async fn handle_delete_node(
    operations: &Arc<NodeOperations>,
    params: Value,
) -> Result<Value, MCPError> {
    let params: DeleteNodeParams = serde_json::from_value(params)
        .map_err(|e| MCPError::invalid_params(format!("Invalid parameters: {}", e)))?;

    // Delete node via NodeOperations
    let result = operations
        .delete_node(&params.node_id)
        .await
        .map_err(|e| MCPError::node_delete_failed(format!("Failed to delete node: {}", e)))?;

    Ok(json!({
        "node_id": params.node_id,
        "existed": result.existed,
        "success": true
    }))
}

/// Handle query_nodes MCP request
pub async fn handle_query_nodes(
    operations: &Arc<NodeOperations>,
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

    // Query nodes via NodeOperations
    let nodes = operations
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

// =========================================================================
// Helper Functions for Index-Based Operations
// =========================================================================

/// Check if a string is a valid date format (YYYY-MM-DD)
fn is_valid_date_format(s: &str) -> bool {
    NaiveDate::parse_from_str(s, "%Y-%m-%d").is_ok()
}

/// Ensure parent node exists, auto-creating date nodes if needed
async fn ensure_parent_exists(
    operations: &Arc<NodeOperations>,
    parent_id: &str,
) -> Result<(), MCPError> {
    // Check if parent already exists
    if operations
        .get_node(parent_id)
        .await
        .map_err(|e| MCPError::internal_error(format!("Failed to check parent: {}", e)))?
        .is_some()
    {
        return Ok(());
    }

    // If parent_id looks like a date (YYYY-MM-DD), auto-create date node
    if is_valid_date_format(parent_id) {
        // Try to create - date nodes use their content as ID
        let _ = operations
            .create_node(
                "date".to_string(),
                parent_id.to_string(),
                None,
                None,
                None,
                json!({}),
            )
            .await
            .map_err(|e| {
                MCPError::node_creation_failed(format!("Failed to auto-create date node: {}", e))
            })?;

        Ok(())
    } else {
        Err(MCPError::invalid_params(format!(
            "Parent node '{}' not found",
            parent_id
        )))
    }
}

/// Get all children of a parent in order by following the sibling chain
///
/// Performance: O(n) using HashMap for constant-time lookups instead of O(n²) with repeated .find()
async fn get_children_ordered(
    operations: &Arc<NodeOperations>,
    parent_id: &str,
    include_content: bool,
) -> Result<Vec<ChildInfo>, MCPError> {
    // 1. Query all children with parent_id
    let children = operations
        .query_nodes(NodeFilter::new().with_parent_id(parent_id.to_string()))
        .await
        .map_err(|e| MCPError::internal_error(format!("Failed to query children: {}", e)))?;

    // 2. Build HashMap for O(1) lookups by before_sibling_id
    //    Map: before_sibling_id -> Node (enables constant-time chain following)
    let mut children_map: HashMap<Option<String>, &crate::models::Node> = HashMap::new();
    for child in &children {
        children_map.insert(child.before_sibling_id.clone(), child);
    }

    // 3. Follow the linked list from first to last
    let mut ordered = Vec::new();
    let mut current_before: Option<String> = None; // Start with first (before_sibling_id=None)

    while let Some(node) = children_map.get(&current_before) {
        ordered.push(ChildInfo {
            index: ordered.len(),
            node_id: node.id.clone(),
            node_type: node.node_type.clone(),
            content: if include_content {
                Some(node.content.clone())
            } else {
                None
            },
        });
        current_before = Some(node.id.clone());

        // Infinite loop protection (circular chain detection)
        if ordered.len() > children.len() {
            return Err(MCPError::internal_error(format!(
                "Circular sibling chain detected in parent '{}' - data corruption",
                parent_id
            )));
        }
    }

    Ok(ordered)
}

/// Build tree node recursively
fn build_tree_node<'a>(
    operations: &'a Arc<NodeOperations>,
    node: crate::models::Node,
    depth: usize,
    max_depth: usize,
    include_content: bool,
    include_metadata: bool,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<TreeNode, MCPError>> + Send + 'a>> {
    Box::pin(async move {
        // Get children if we haven't reached max depth
        let children = if depth < max_depth {
            let child_infos = get_children_ordered(operations, &node.id, false).await?;

            let mut tree_children = Vec::new();
            for child_info in child_infos {
                // Get full node data for each child
                let child_node = operations
                    .get_node(&child_info.node_id)
                    .await
                    .map_err(|e| {
                        MCPError::internal_error(format!("Failed to get child node: {}", e))
                    })?
                    .ok_or_else(|| {
                        MCPError::internal_error(format!(
                            "Child node '{}' not found",
                            child_info.node_id
                        ))
                    })?;

                // Recursively build child tree
                let tree_child = build_tree_node(
                    operations,
                    child_node,
                    depth + 1,
                    max_depth,
                    include_content,
                    include_metadata,
                )
                .await?;
                tree_children.push(tree_child);
            }
            tree_children
        } else {
            Vec::new()
        };

        let child_count = children.len();

        Ok(TreeNode {
            node_id: node.id.clone(),
            node_type: node.node_type.clone(),
            depth,
            parent_id: node.parent_id.clone(),
            before_sibling_id: node.before_sibling_id.clone(),
            child_count,
            children,
            content: if include_content {
                Some(node.content.clone())
            } else {
                None
            },
            created_at: if include_metadata {
                Some(node.created_at.to_rfc3339())
            } else {
                None
            },
            modified_at: if include_metadata {
                Some(node.modified_at.to_rfc3339())
            } else {
                None
            },
            properties: if include_metadata {
                Some(node.properties.clone())
            } else {
                None
            },
        })
    })
}

// =========================================================================
// Index-Based Child Operation Handlers
// =========================================================================

/// Handle get_children MCP request
pub async fn handle_get_children(
    operations: &Arc<NodeOperations>,
    params: Value,
) -> Result<Value, MCPError> {
    let params: GetChildrenParams = serde_json::from_value(params)
        .map_err(|e| MCPError::invalid_params(format!("Invalid parameters: {}", e)))?;

    // Get children in order
    let children =
        get_children_ordered(operations, &params.parent_id, params.include_content).await?;

    let child_count = children.len();

    Ok(json!({
        "parent_id": params.parent_id,
        "child_count": child_count,
        "children": children
    }))
}

/// Handle insert_child_at_index MCP request
pub async fn handle_insert_child_at_index(
    operations: &Arc<NodeOperations>,
    params: Value,
) -> Result<Value, MCPError> {
    let params: InsertChildAtIndexParams = serde_json::from_value(params)
        .map_err(|e| MCPError::invalid_params(format!("Invalid parameters: {}", e)))?;

    // 1. Ensure parent exists (auto-create if date format)
    ensure_parent_exists(operations, &params.parent_id).await?;

    // 2. Get parent to determine container_node_id
    let parent = operations
        .get_node(&params.parent_id)
        .await
        .map_err(|e| MCPError::internal_error(format!("Failed to get parent: {}", e)))?
        .ok_or_else(|| {
            MCPError::invalid_params(format!("Parent node '{}' not found", params.parent_id))
        })?;

    // 3. Get all siblings in order
    let children_info = get_children_ordered(operations, &params.parent_id, false).await?;

    // 4. Calculate before_sibling_id based on index
    // Note: before_sibling_id = the node that comes immediately BEFORE this one
    // - Index 0 → before_sibling_id = None (first position, nothing before it)
    // - Index 1 → before_sibling_id = children[0] (after first node)
    // - Index >= length → before_sibling_id = children[last] (append at end)
    let before_sibling_id = if params.index == 0 {
        None // Insert at beginning
    } else if params.index >= children_info.len() {
        // Append at end: set before_sibling_id to last child
        children_info.last().map(|c| c.node_id.clone())
    } else {
        // Insert at position: before_sibling_id = node at (index - 1)
        Some(children_info[params.index - 1].node_id.clone())
    };

    // 5. Determine container_node_id (from parent or parent itself if container)
    let container_node_id = if parent.is_root() {
        Some(parent.id.clone()) // Parent is the container
    } else {
        parent.container_node_id // Inherit parent's container
    };

    // 6. Create node using pointer-based operation
    let node_id = operations
        .create_node(
            params.node_type.clone(),
            params.content,
            Some(params.parent_id.clone()),
            container_node_id,
            before_sibling_id.clone(),
            params.properties,
        )
        .await
        .map_err(|e| MCPError::node_creation_failed(format!("Failed to create node: {}", e)))?;

    // 7. Fix sibling chain after insertion
    // When inserting at a specific position, we need to update the node that was previously
    // at that position to now point to the newly inserted node
    if params.index < children_info.len() {
        // There's a node that should come AFTER the new node
        // Update it to point to the new node
        let node_to_update_id = &children_info[params.index].node_id;

        // Use reorder_node to update the old node's before_sibling_id to point to new node
        operations
            .reorder_node(node_to_update_id, Some(&node_id))
            .await
            .map_err(|e| {
                MCPError::node_update_failed(format!("Failed to fix sibling chain: {}", e))
            })?;
    }

    Ok(json!({
        "node_id": node_id,
        "parent_id": params.parent_id,
        "index": params.index,
        "node_type": params.node_type
    }))
}

/// Handle move_child_to_index MCP request
pub async fn handle_move_child_to_index(
    operations: &Arc<NodeOperations>,
    params: Value,
) -> Result<Value, MCPError> {
    let params: MoveChildToIndexParams = serde_json::from_value(params)
        .map_err(|e| MCPError::invalid_params(format!("Invalid parameters: {}", e)))?;

    // 1. Get the node to move
    let node = operations
        .get_node(&params.node_id)
        .await
        .map_err(|e| MCPError::internal_error(format!("Failed to get node: {}", e)))?
        .ok_or_else(|| MCPError::invalid_params(format!("Node '{}' not found", params.node_id)))?;

    let parent_id = node.parent_id.ok_or_else(|| {
        MCPError::invalid_params("Node has no parent (cannot reorder root nodes)".to_string())
    })?;

    // 2. Get all siblings in current order (excluding the node being moved)
    let all_children = get_children_ordered(operations, &parent_id, false).await?;
    let siblings: Vec<_> = all_children
        .into_iter()
        .filter(|c| c.node_id != params.node_id)
        .collect();

    // 3. Calculate new before_sibling_id
    // Note: before_sibling_id semantics are "insert AFTER this node"
    // - None means "first node" (no node before it)
    // - Some(last_node_id) means "after last node" (append at end)
    let before_sibling_id = if params.index >= siblings.len() {
        // Move to end: set before_sibling_id to last sibling
        siblings.last().map(|s| s.node_id.clone())
    } else if params.index == 0 {
        // Move to beginning: before_sibling_id = None (first position)
        None
    } else {
        // Insert at specific position: before_sibling_id = node at (index - 1)
        Some(siblings[params.index - 1].node_id.clone())
    };

    // 4. Use reorder_node operation (which now handles sibling chain integrity)
    operations
        .reorder_node(&params.node_id, before_sibling_id.as_deref())
        .await
        .map_err(|e| MCPError::node_update_failed(format!("Failed to reorder node: {}", e)))?;

    Ok(json!({
        "node_id": params.node_id,
        "new_index": params.index,
        "parent_id": parent_id
    }))
}

/// Handle get_child_at_index MCP request
pub async fn handle_get_child_at_index(
    operations: &Arc<NodeOperations>,
    params: Value,
) -> Result<Value, MCPError> {
    let params: GetChildAtIndexParams = serde_json::from_value(params)
        .map_err(|e| MCPError::invalid_params(format!("Invalid parameters: {}", e)))?;

    // Get all children in order
    let children =
        get_children_ordered(operations, &params.parent_id, params.include_content).await?;

    // Get child at index
    let child = children.get(params.index).ok_or_else(|| {
        MCPError::invalid_params(format!(
            "Index {} out of bounds (parent has {} children)",
            params.index,
            children.len()
        ))
    })?;

    Ok(json!({
        "index": child.index,
        "node_id": &child.node_id,
        "node_type": &child.node_type,
        "content": child.content,
        "parent_id": params.parent_id
    }))
}

/// Handle get_node_tree MCP request
pub async fn handle_get_node_tree(
    operations: &Arc<NodeOperations>,
    params: Value,
) -> Result<Value, MCPError> {
    let params: GetNodeTreeParams = serde_json::from_value(params)
        .map_err(|e| MCPError::invalid_params(format!("Invalid parameters: {}", e)))?;

    // Validate max_depth
    if params.max_depth == 0 || params.max_depth > 100 {
        return Err(MCPError::invalid_params(format!(
            "max_depth must be between 1 and 100, got {}",
            params.max_depth
        )));
    }

    let root = operations
        .get_node(&params.node_id)
        .await
        .map_err(|e| MCPError::internal_error(format!("Failed to get node: {}", e)))?
        .ok_or_else(|| MCPError::invalid_params(format!("Node '{}' not found", params.node_id)))?;

    let tree = build_tree_node(
        operations,
        root,
        0,
        params.max_depth,
        params.include_content,
        params.include_metadata,
    )
    .await?;

    Ok(json!(tree))
}

// Include tests
#[cfg(test)]
#[path = "nodes_test.rs"]
mod nodes_test;
