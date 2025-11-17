//! MCP Node CRUD Handlers
//!
//! Wraps NodeOperations for MCP protocol access.
//! Pure business logic - no Tauri dependencies.

use crate::mcp::types::MCPError;
use crate::models::schema::ProtectionLevel;
use crate::models::{NodeFilter, OrderBy};
use crate::operations::{CreateNodeParams, NodeOperationError, NodeOperations};
use crate::services::SchemaService;
use chrono::NaiveDate;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Arc;

/// Convert NodeOperationError to MCPError with proper formatting
///
/// Special handling for VersionConflict errors: includes current node state
/// in the error response for client-side merge.
fn operation_error_to_mcp(error: NodeOperationError) -> MCPError {
    match error {
        NodeOperationError::VersionConflict {
            node_id,
            expected_version,
            actual_version,
            current_node,
        } => {
            // current_node is already available in the error as Box<Node>
            let current_node_value = serde_json::to_value(&*current_node).ok();
            MCPError::version_conflict(
                node_id,
                expected_version,
                actual_version,
                current_node_value,
            )
        }
        NodeOperationError::NodeNotFound { node_id } => MCPError::node_not_found(&node_id),
        NodeOperationError::InvalidOperation { reason } => MCPError::validation_error(reason),
        NodeOperationError::DatabaseError(source) => {
            MCPError::internal_error(format!("Database error: {}", source))
        }
        // Handle any other error variants
        _ => MCPError::internal_error(format!("Operation error: {}", error)),
    }
}

/// Parameters for create_node method from MCP clients
///
/// Note: MCP clients don't provide IDs - they are generated server-side.
/// This struct is used for deserialization from MCP requests.
#[derive(Debug, Deserialize)]
pub struct MCPCreateNodeParams {
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
    /// Expected version for optimistic concurrency control (REQUIRED)
    /// This prevents race conditions and silent data loss from concurrent updates.
    /// Always fetch the node first to get its current version before updating.
    pub version: i64,
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
    /// Expected version for optimistic concurrency control (REQUIRED)
    /// This prevents race conditions and accidental deletion of modified nodes.
    /// Always fetch the node first to get its current version before deleting.
    pub version: i64,
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
    /// Expected version for optimistic concurrency control
    pub version: i64,
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
    let mcp_params: MCPCreateNodeParams = serde_json::from_value(params)
        .map_err(|e| MCPError::invalid_params(format!("Invalid parameters: {}", e)))?;

    // Create node via NodeOperations (enforces all business rules)
    // Note: container_node_id is auto-derived from parent chain by backend
    let node_id = operations
        .create_node(CreateNodeParams {
            id: None, // MCP generates IDs server-side
            node_type: mcp_params.node_type.clone(),
            content: mcp_params.content,
            parent_id: mcp_params.parent_id,
            before_sibling_id: mcp_params.before_sibling_id,
            properties: mcp_params.properties,
        })
        .await
        .map_err(|e| MCPError::node_creation_failed(format!("Failed to create node: {}", e)))?;

    Ok(json!({
        "node_id": node_id,
        "node_type": mcp_params.node_type,
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
    schema_service: &Arc<SchemaService>,
    params: Value,
) -> Result<Value, MCPError> {
    let params: UpdateNodeParams = serde_json::from_value(params)
        .map_err(|e| MCPError::invalid_params(format!("Invalid parameters: {}", e)))?;

    // Validate schema changes if properties are being updated
    if let Some(ref new_properties) = params.properties {
        // Check if this node's type has a schema
        if let Ok(Some(existing_node)) = operations.get_node(&params.node_id).await {
            if let Ok(schema) = schema_service.get_schema(&existing_node.node_type).await {
                // Get old properties for comparison (with proper ownership)
                let empty_map = serde_json::Map::new();
                let old_properties = existing_node.properties.as_object().unwrap_or(&empty_map);
                let new_props_obj = new_properties.as_object().ok_or_else(|| {
                    MCPError::invalid_params("properties must be an object".to_string())
                })?;

                // Check for core field deletions
                for field in &schema.fields {
                    if field.protection == ProtectionLevel::Core {
                        let old_has_field = old_properties.contains_key(&field.name);
                        let new_has_field = new_props_obj.contains_key(&field.name);

                        if old_has_field && !new_has_field {
                            return Err(MCPError::invalid_params(format!(
                                "Cannot delete core field '{}' from schema '{}'",
                                field.name, existing_node.node_type
                            )));
                        }
                    }
                }

                // Check for core field type changes (detect by checking if new value type differs from schema type)
                // This is a basic check - full type validation would require more complex logic
                for field in &schema.fields {
                    if field.protection == ProtectionLevel::Core {
                        if let Some(new_value) = new_props_obj.get(&field.name) {
                            // Basic type validation: ensure enum fields only have allowed values
                            if field.field_type == "enum" {
                                if let Some(value_str) = new_value.as_str() {
                                    let allowed_values =
                                        schema.get_enum_values(&field.name).unwrap_or_default();
                                    if !allowed_values.contains(&value_str.to_string()) {
                                        // Check if it's trying to set a value that's not in core or user values
                                        return Err(MCPError::invalid_params(format!(
                                            "Invalid enum value '{}' for field '{}'. Allowed values: {:?}",
                                            value_str, field.name, allowed_values
                                        )));
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    // Update node via NodeOperations (enforces Rule 5: content updates only, no hierarchy changes)
    //
    // MCP update_node intentionally restricts certain fields for data integrity:
    // - parent_id: Use move_node operation for parent changes
    // - container_node_id: Auto-calculated with parent changes
    // - before_sibling_id: Use reorder_node operation for sibling changes
    // - embedding_vector: Embeddings are auto-generated from content via background jobs
    //
    // Use MCP only for content/property updates. Use separate operations for structural changes.

    // Version is now mandatory - no auto-fetch to prevent TOCTOU race conditions
    let updated_node = operations
        .update_node(
            &params.node_id,
            params.version,
            params.content,
            params.node_type,
            params.properties,
        )
        .await
        .map_err(operation_error_to_mcp)?;

    Ok(json!({
        "node_id": params.node_id,
        "version": updated_node.version,
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
    // Version is now mandatory - no auto-fetch to prevent TOCTOU race conditions
    let result = operations
        .delete_node(&params.node_id, params.version)
        .await
        .map_err(operation_error_to_mcp)?;

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

    // Note: parent_id and container_node_id filters removed in graph-native refactor
    // These parameters are kept in the MCP API for backward compatibility but ignored
    // Clients should use graph queries to traverse relationships instead
    if params.parent_id.is_some() {
        tracing::warn!("parent_id filter ignored - use graph queries for relationship traversal");
    }

    if params.container_node_id.is_some() {
        tracing::warn!(
            "container_node_id filter ignored - use graph queries for relationship traversal"
        );
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
            .create_node(CreateNodeParams {
                id: None, // MCP generates IDs server-side
                node_type: "date".to_string(),
                content: parent_id.to_string(),
                parent_id: None,
                before_sibling_id: None,
                properties: json!({}),
            })
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
    // 1. Query all children using graph traversal
    // In graph-native architecture, we use outgoing edges to find children
    let children = operations
        .get_children(parent_id)
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

        // Get parent_id using graph traversal for API response
        // TODO: Implement proper parent lookup - for now return None
        let parent_id: Option<String> = None;

        Ok(TreeNode {
            node_id: node.id.clone(),
            node_type: node.node_type.clone(),
            depth,
            parent_id,
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

    // 2. Get parent to verify it exists (hierarchy managed via edges)
    let _parent = operations
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

    // 5. Create node using pointer-based operation
    // Note: container/root is auto-derived from parent chain by backend
    let node_id = operations
        .create_node(CreateNodeParams {
            id: None, // MCP generates IDs server-side
            node_type: params.node_type.clone(),
            content: params.content,
            parent_id: Some(params.parent_id.clone()),
            before_sibling_id: before_sibling_id.clone(),
            properties: params.properties,
        })
        .await
        .map_err(|e| MCPError::node_creation_failed(format!("Failed to create node: {}", e)))?;

    // 6. Fix sibling chain after insertion
    // When inserting at a specific position, we need to update the node that was previously
    // at that position to now point to the newly inserted node
    if params.index < children_info.len() {
        // There's a node that should come AFTER the new node
        // Update it to point to the new node
        let node_to_update_id = &children_info[params.index].node_id;

        // Get current version for the node being reordered (side effect of insertion)
        // Note: This is not passed by client since client doesn't control this sibling
        let node_to_update = operations
            .get_node(node_to_update_id)
            .await
            .map_err(|e| MCPError::node_not_found(&format!("Node not found: {}", e)))?
            .ok_or_else(|| MCPError::node_not_found(node_to_update_id))?;

        // Use reorder_node to update the old node's before_sibling_id to point to new node
        operations
            .reorder_node(node_to_update_id, node_to_update.version, Some(&node_id))
            .await
            .map_err(operation_error_to_mcp)?;
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

    // 1. Verify the node exists
    let _node = operations
        .get_node(&params.node_id)
        .await
        .map_err(|e| MCPError::internal_error(format!("Failed to get node: {}", e)))?
        .ok_or_else(|| MCPError::invalid_params(format!("Node '{}' not found", params.node_id)))?;

    // Get parent using graph traversal
    let parent_id = operations
        .get_parent_id(&params.node_id)
        .await
        .map_err(|e| MCPError::internal_error(format!("Failed to get parent: {}", e)))?
        .ok_or_else(|| {
            MCPError::invalid_params(format!(
                "Node '{}' has no parent (is a root node) - cannot reorder",
                params.node_id
            ))
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
        .reorder_node(
            &params.node_id,
            params.version,
            before_sibling_id.as_deref(),
        )
        .await
        .map_err(operation_error_to_mcp)?;

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

// =========================================================================
// Batch Operations for Performance
// =========================================================================

/// Parameters for get_nodes_batch method
#[derive(Debug, Deserialize)]
pub struct GetNodesBatchParams {
    pub node_ids: Vec<String>,
}

/// Result of batch get operation
#[derive(Debug, serde::Serialize)]
pub struct BatchGetResult {
    /// Successfully retrieved nodes
    nodes: Vec<serde_json::Value>,
    /// IDs of nodes that weren't found
    not_found: Vec<String>,
}

/// Get multiple nodes in a single request
///
/// More efficient than calling get_node multiple times when AI needs
/// to fetch details for many nodes (e.g., after parsing markdown export).
///
/// # Example
///
/// ```rust,no_run
/// let params = json!({
///     "node_ids": ["task-1", "task-2", "task-3"]
/// });
/// let result = handle_get_nodes_batch(&operations, params).await?;
/// // Returns all found nodes + list of IDs that don't exist
/// ```
pub async fn handle_get_nodes_batch(
    operations: &Arc<NodeOperations>,
    params: Value,
) -> Result<Value, MCPError> {
    // Parse parameters
    let params: GetNodesBatchParams = serde_json::from_value(params)
        .map_err(|e| MCPError::invalid_params(format!("Invalid parameters: {}", e)))?;

    // Validate input
    if params.node_ids.is_empty() {
        return Err(MCPError::invalid_params(
            "node_ids cannot be empty".to_string(),
        ));
    }

    if params.node_ids.len() > 100 {
        return Err(MCPError::invalid_params(format!(
            "Batch size exceeds maximum of 100 nodes (got {} nodes)",
            params.node_ids.len()
        )));
    }

    // Fetch all nodes
    let mut nodes = Vec::new();
    let mut not_found = Vec::new();

    for node_id in params.node_ids {
        match operations.get_node(&node_id).await {
            Ok(Some(node)) => {
                nodes.push(serde_json::to_value(&node).unwrap());
            }
            Ok(None) => {
                not_found.push(node_id);
            }
            Err(e) => {
                tracing::warn!("Error fetching node {}: {}", node_id, e);
                not_found.push(node_id);
            }
        }
    }

    Ok(json!({
        "nodes": nodes,
        "not_found": not_found,
        "count": nodes.len()
    }))
}

/// Single update in a batch request
#[derive(Debug, Deserialize)]
pub struct BatchUpdateItem {
    /// Node ID to update
    pub id: String,
    /// Expected version for optimistic concurrency control
    /// If not provided, will fetch current version (optimistic: assumes no conflict)
    #[serde(default)]
    pub version: Option<i64>,
    /// Optional updated content
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    /// Optional updated node type
    #[serde(skip_serializing_if = "Option::is_none")]
    pub node_type: Option<String>,
    /// Optional updated properties
    #[serde(skip_serializing_if = "Option::is_none")]
    pub properties: Option<Value>,
}

/// Parameters for update_nodes_batch
#[derive(Debug, Deserialize)]
pub struct UpdateNodesBatchParams {
    pub updates: Vec<BatchUpdateItem>,
}

/// Failed update info
#[derive(Debug, serde::Serialize)]
pub struct BatchUpdateFailure {
    id: String,
    error: String,
}

/// Update multiple nodes in a single request
///
/// Applies updates sequentially. If some updates fail,
/// returns both successful and failed update IDs for client handling.
///
/// # Example
///
/// ```rust,no_run
/// let params = json!({
///     "updates": [
///         { "id": "task-1", "content": "- [x] Done" },
///         { "id": "task-2", "properties": { "priority": "high" } }
///     ]
/// });
/// let result = handle_update_nodes_batch(&operations, &schema_service, params).await?;
/// ```
pub async fn handle_update_nodes_batch(
    operations: &Arc<NodeOperations>,
    schema_service: &Arc<SchemaService>,
    params: Value,
) -> Result<Value, MCPError> {
    // Parse parameters
    let params: UpdateNodesBatchParams = serde_json::from_value(params)
        .map_err(|e| MCPError::invalid_params(format!("Invalid parameters: {}", e)))?;

    // Validate input
    if params.updates.is_empty() {
        return Err(MCPError::invalid_params(
            "updates cannot be empty".to_string(),
        ));
    }

    if params.updates.len() > 100 {
        return Err(MCPError::invalid_params(format!(
            "Batch size exceeds maximum of 100 updates (got {} updates)",
            params.updates.len()
        )));
    }

    // Apply all updates
    let mut updated = Vec::new();
    let mut failed: Vec<BatchUpdateFailure> = Vec::new();

    for update in params.updates {
        // If version not provided, fetch current version (optimistic: assumes no concurrent updates)
        // ⚠️ WARNING: This bypasses optimistic concurrency control!
        let version = match update.version {
            Some(v) => v,
            None => {
                tracing::warn!(
                    "OCC bypassed: version parameter not provided for batch update (race condition possible)"
                );
                match operations.get_node(&update.id).await {
                    Ok(Some(node)) => node.version,
                    Ok(None) => {
                        failed.push(BatchUpdateFailure {
                            id: update.id.clone(),
                            error: format!("Node '{}' does not exist", update.id),
                        });
                        continue;
                    }
                    Err(e) => {
                        tracing::warn!("Failed to fetch node {} for version: {}", update.id, e);
                        let mcp_error = operation_error_to_mcp(e);
                        failed.push(BatchUpdateFailure {
                            id: update.id,
                            error: mcp_error.message,
                        });
                        continue;
                    }
                }
            }
        };

        // Validate schema changes if properties are being updated
        if let Some(ref new_properties) = update.properties {
            if let Ok(Some(existing_node)) = operations.get_node(&update.id).await {
                if let Ok(schema) = schema_service.get_schema(&existing_node.node_type).await {
                    let empty_map = serde_json::Map::new();
                    let old_properties = existing_node.properties.as_object().unwrap_or(&empty_map);

                    if let Some(new_props_obj) = new_properties.as_object() {
                        // Check for core field deletions
                        for field in &schema.fields {
                            if field.protection == ProtectionLevel::Core {
                                let old_has_field = old_properties.contains_key(&field.name);
                                let new_has_field = new_props_obj.contains_key(&field.name);

                                if old_has_field && !new_has_field {
                                    failed.push(BatchUpdateFailure {
                                        id: update.id.clone(),
                                        error: format!(
                                            "Cannot delete core field '{}' from schema '{}'",
                                            field.name, existing_node.node_type
                                        ),
                                    });
                                    continue;
                                }
                            }

                            // Validate enum values
                            if field.protection == ProtectionLevel::Core
                                && field.field_type == "enum"
                            {
                                if let Some(new_value) = new_props_obj.get(&field.name) {
                                    if let Some(value_str) = new_value.as_str() {
                                        let allowed_values =
                                            schema.get_enum_values(&field.name).unwrap_or_default();
                                        if !allowed_values.contains(&value_str.to_string()) {
                                            failed.push(BatchUpdateFailure {
                                                id: update.id.clone(),
                                                error: format!(
                                                    "Invalid enum value '{}' for field '{}'. Allowed values: {:?}",
                                                    value_str, field.name, allowed_values
                                                ),
                                            });
                                            continue;
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        // Apply update via NodeOperations (enforces all business rules)
        match operations
            .update_node(
                &update.id,
                version,
                update.content,
                update.node_type,
                update.properties,
            )
            .await
        {
            Ok(_) => {
                updated.push(update.id);
            }
            Err(e) => {
                tracing::warn!("Failed to update node {}: {}", update.id, e);
                let mcp_error = operation_error_to_mcp(e);
                failed.push(BatchUpdateFailure {
                    id: update.id,
                    error: mcp_error.message,
                });
            }
        }
    }

    Ok(json!({
        "updated": updated,
        "failed": failed,
        "count": updated.len()
    }))
}

// Include tests
#[cfg(test)]
#[path = "nodes_test.rs"]
mod nodes_test;
