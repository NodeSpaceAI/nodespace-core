//! MCP Node CRUD Handlers
//!
//! Wraps NodeService for MCP protocol access.
//! Pure business logic - no Tauri dependencies.
//!
//! As of Issue #676, all handlers use NodeService directly instead of NodeOperations.

use crate::mcp::types::MCPError;
use crate::models::{Node, NodeFilter, NodeUpdate, OrderBy};
use crate::services::{NodeService, NodeServiceError};
use chrono::NaiveDate;
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;

/// Convert a Node to its strongly-typed JSON representation
///
/// Delegates to the canonical `models::node_to_typed_value` and maps errors to MCPError.
fn node_to_typed_value(node: Node) -> Result<Value, MCPError> {
    crate::models::node_to_typed_value(node).map_err(MCPError::internal_error)
}

/// Convert a list of Nodes to their strongly-typed JSON representations
fn nodes_to_typed_values(nodes: Vec<Node>) -> Result<Vec<Value>, MCPError> {
    crate::models::nodes_to_typed_values(nodes).map_err(MCPError::internal_error)
}

/// Convert NodeServiceError to MCPError with proper formatting
///
/// Special handling for VersionConflict errors to help client-side merge.
fn service_error_to_mcp(error: NodeServiceError) -> MCPError {
    match error {
        NodeServiceError::VersionConflict {
            node_id,
            expected_version,
            actual_version,
        } => MCPError::version_conflict(node_id, expected_version, actual_version, None),
        NodeServiceError::NodeNotFound { id } => MCPError::node_not_found(&id),
        NodeServiceError::ValidationFailed(e) => MCPError::validation_error(e.to_string()),
        NodeServiceError::InvalidParent { parent_id } => {
            MCPError::validation_error(format!("Invalid parent: {}", parent_id))
        }
        NodeServiceError::InvalidRoot { root_node_id } => {
            MCPError::validation_error(format!("Invalid root: {}", root_node_id))
        }
        NodeServiceError::CircularReference { context } => {
            MCPError::validation_error(format!("Circular reference: {}", context))
        }
        NodeServiceError::HierarchyViolation(msg) => {
            MCPError::validation_error(format!("Hierarchy violation: {}", msg))
        }
        NodeServiceError::DatabaseError(e) => {
            MCPError::internal_error(format!("Database error: {}", e))
        }
        _ => MCPError::internal_error(format!("Service error: {}", error)),
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
    pub root_id: Option<String>,
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
    /// Expected version for optimistic concurrency control (optional for MCP)
    /// If provided, enables OCC to prevent race conditions.
    /// If omitted, fetches current version automatically (convenient for AI agents).
    #[serde(default)]
    pub version: Option<i64>,
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
    pub root_id: Option<String>,
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
pub async fn handle_create_node<C>(
    node_service: &Arc<NodeService<C>>,
    params: Value,
) -> Result<Value, MCPError>
where
    C: surrealdb::Connection,
{
    let mcp_params: MCPCreateNodeParams = serde_json::from_value(params)
        .map_err(|e| MCPError::invalid_params(format!("Invalid parameters: {}", e)))?;

    // Create node via NodeService (enforces all business rules)
    // Note: root_id is auto-derived from parent chain by backend
    let parent_id = mcp_params.parent_id.clone();
    let node_id = node_service
        .create_node_with_parent(crate::services::CreateNodeParams {
            id: None, // MCP generates IDs server-side
            node_type: mcp_params.node_type.clone(),
            content: mcp_params.content,
            parent_id: mcp_params.parent_id,
            insert_after_node_id: None, // Insert at beginning (this endpoint doesn't expose positioning)
            properties: mcp_params.properties,
        })
        .await
        .map_err(|e| MCPError::node_creation_failed(format!("Failed to create node: {}", e)))?;

    // Fetch the created node for response (includes version, timestamps, etc.)
    let created_node = node_service
        .get_node(&node_id)
        .await
        .map_err(|e| MCPError::internal_error(format!("Failed to fetch created node: {}", e)))?
        .ok_or_else(|| MCPError::internal_error("Created node not found".to_string()))?;

    let node_data = node_to_typed_value(created_node)?;

    Ok(json!({
        "node_id": node_id,
        "node_type": mcp_params.node_type,
        "parent_id": parent_id,
        "success": true,
        "node_data": node_data
    }))
}

/// Handle get_node MCP request
///
/// Returns strongly-typed structs for types with spoke tables (task, schema),
/// and generic Node for simple types (text, header, etc.).
///
/// This provides compile-time type safety for complex types while maintaining
/// flexibility for simple content-only types.
pub async fn handle_get_node<C>(
    node_service: &Arc<NodeService<C>>,
    params: Value,
) -> Result<Value, MCPError>
where
    C: surrealdb::Connection,
{
    let params: GetNodeParams = serde_json::from_value(params)
        .map_err(|e| MCPError::invalid_params(format!("Invalid parameters: {}", e)))?;

    // Fetch the node (get_node already fetches spoke data for task/schema types)
    let node = node_service
        .get_node(&params.node_id)
        .await
        .map_err(|e| MCPError::internal_error(format!("Failed to get node: {}", e)))?
        .ok_or_else(|| MCPError::node_not_found(&params.node_id))?;

    // Convert to strongly-typed JSON representation
    node_to_typed_value(node)
}

/// Handle update_node MCP request
///
/// Schema validation is handled by NodeService::validate_node_against_schema(),
/// which validates enum values and required fields. No SchemaService needed.
pub async fn handle_update_node<C>(
    node_service: &Arc<NodeService<C>>,
    params: Value,
) -> Result<Value, MCPError>
where
    C: surrealdb::Connection,
{
    let params: UpdateNodeParams = serde_json::from_value(params)
        .map_err(|e| MCPError::invalid_params(format!("Invalid parameters: {}", e)))?;

    // Update node via NodeService (enforces Rule 5: content updates only, no hierarchy changes)
    // NodeService.update_node_with_occ validates against schema automatically
    //
    // MCP update_node intentionally restricts certain fields for data integrity:
    // - parent_id: Use move_node operation for parent changes
    // - root_id: Auto-calculated with parent changes
    // - before_sibling_id: Use reorder_node operation for sibling changes
    // - embedding_vector: Embeddings are auto-generated from content via background jobs
    //
    // Use MCP only for content/property updates. Use separate operations for structural changes.

    // Build NodeUpdate from params
    // Note: Embeddings are auto-generated via root-aggregate model (Issue #729)
    let update = NodeUpdate {
        content: params.content,
        node_type: params.node_type,
        properties: params.properties,
    };

    // If version not provided, fetch current version (convenient for AI agents)
    // If version provided, use OCC for concurrent update protection
    let version = match params.version {
        Some(v) => v,
        None => {
            let node = node_service
                .get_node(&params.node_id)
                .await
                .map_err(|e| MCPError::internal_error(format!("Failed to get node: {}", e)))?
                .ok_or_else(|| MCPError::node_not_found(&params.node_id))?;
            node.version
        }
    };

    let updated_node = match node_service
        .update_node_with_occ(&params.node_id, version, update)
        .await
    {
        Ok(node) => node,
        Err(NodeServiceError::VersionConflict {
            node_id,
            expected_version,
            actual_version,
        }) => {
            // Fetch current node state to include in error response for client-side merge
            let current_node = node_service
                .get_node(&node_id)
                .await
                .ok()
                .flatten()
                .and_then(|n| serde_json::to_value(&n).ok());
            return Err(MCPError::version_conflict(
                node_id,
                expected_version,
                actual_version,
                current_node,
            ));
        }
        Err(e) => return Err(service_error_to_mcp(e)),
    };

    // Include full node data in response for:
    // 1. SSE broadcasting (callback can extract node_data)
    // 2. Client convenience (no need for separate fetch)
    let node_data = node_to_typed_value(updated_node)?;

    Ok(json!({
        "node_id": params.node_id,
        "version": node_data.get("version").and_then(|v| v.as_i64()).unwrap_or(0),
        "success": true,
        "node_data": node_data
    }))
}

/// Handle delete_node MCP request
pub async fn handle_delete_node<C>(
    node_service: &Arc<NodeService<C>>,
    params: Value,
) -> Result<Value, MCPError>
where
    C: surrealdb::Connection,
{
    let params: DeleteNodeParams = serde_json::from_value(params)
        .map_err(|e| MCPError::invalid_params(format!("Invalid parameters: {}", e)))?;

    // Delete node via NodeService
    // Version is now mandatory - no auto-fetch to prevent TOCTOU race conditions
    let result = node_service
        .delete_node_with_occ(&params.node_id, params.version)
        .await
        .map_err(service_error_to_mcp)?;

    Ok(json!({
        "node_id": params.node_id,
        "existed": result.existed,
        "success": true
    }))
}

/// Handle query_nodes MCP request
pub async fn handle_query_nodes<C>(
    node_service: &Arc<NodeService<C>>,
    params: Value,
) -> Result<Value, MCPError>
where
    C: surrealdb::Connection,
{
    let params: QueryNodesParams = serde_json::from_value(params)
        .map_err(|e| MCPError::invalid_params(format!("Invalid parameters: {}", e)))?;

    // Build NodeFilter using builder pattern
    let mut filter = NodeFilter::new();

    if let Some(node_type) = params.node_type {
        filter = filter.with_node_type(node_type);
    }

    // Note: parent_id and root_id filters removed in graph-native refactor
    // These parameters are kept in the MCP API for backward compatibility but ignored
    // Clients should use graph queries to traverse relationships instead
    if params.parent_id.is_some() {
        tracing::warn!("parent_id filter ignored - use graph queries for relationship traversal");
    }

    // DEPRECATED: root_id filter is ignored in graph-native architecture
    // Kept for backward compatibility but clients should use graph queries
    if params.root_id.is_some() {
        tracing::warn!(
            "root_id filter is deprecated. Filter ignored - use graph queries for relationship traversal"
        );
    }

    if let Some(limit) = params.limit {
        filter = filter.with_limit(limit);
    }

    if let Some(offset) = params.offset {
        filter = filter.with_offset(offset);
    }

    filter = filter.with_order_by(OrderBy::CreatedDesc);

    // Query nodes via NodeService
    let nodes = node_service
        .query_nodes(filter)
        .await
        .map_err(|e| MCPError::internal_error(format!("Failed to query nodes: {}", e)))?;

    // Convert nodes to strongly-typed JSON representations
    let count = nodes.len();
    let typed_nodes = nodes_to_typed_values(nodes)?;

    Ok(json!({
        "nodes": typed_nodes,
        "count": count
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
async fn ensure_parent_exists<C>(
    node_service: &Arc<NodeService<C>>,
    parent_id: &str,
) -> Result<(), MCPError>
where
    C: surrealdb::Connection,
{
    // Check if parent already exists
    if node_service
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
        let _ = node_service
            .create_node_with_parent(crate::services::CreateNodeParams {
                id: None, // MCP generates IDs server-side
                node_type: "date".to_string(),
                content: parent_id.to_string(),
                parent_id: None,
                insert_after_node_id: None,
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

/// Get all children of a parent in order
///
/// In graph-native architecture, children are already ordered by the `order` field
/// on has_child edges (fractional ordering), so we just map them to ChildInfo.
async fn get_children_ordered<C>(
    node_service: &Arc<NodeService<C>>,
    parent_id: &str,
    include_content: bool,
) -> Result<Vec<ChildInfo>, MCPError>
where
    C: surrealdb::Connection,
{
    // In graph-native architecture, get_children() returns children already sorted
    // by the `order` field on has_child edges (fractional ordering)
    let children = node_service
        .get_children(parent_id)
        .await
        .map_err(|e| MCPError::internal_error(format!("Failed to query children: {}", e)))?;

    // Map to ChildInfo with index
    let ordered: Vec<ChildInfo> = children
        .into_iter()
        .enumerate()
        .map(|(index, node)| ChildInfo {
            index,
            node_id: node.id,
            node_type: node.node_type,
            content: if include_content {
                Some(node.content)
            } else {
                None
            },
        })
        .collect();

    Ok(ordered)
}

/// Build tree node recursively
fn build_tree_node<'a, C>(
    node_service: &'a Arc<NodeService<C>>,
    node: crate::models::Node,
    depth: usize,
    max_depth: usize,
    include_content: bool,
    include_metadata: bool,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<TreeNode, MCPError>> + Send + 'a>>
where
    C: surrealdb::Connection + 'a,
{
    Box::pin(async move {
        // Get children if we haven't reached max depth
        let children = if depth < max_depth {
            let child_infos = get_children_ordered(node_service, &node.id, false).await?;

            let mut tree_children = Vec::new();
            for child_info in child_infos {
                // Get full node data for each child
                let child_node = node_service
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
                    node_service,
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
pub async fn handle_get_children<C>(
    node_service: &Arc<NodeService<C>>,
    params: Value,
) -> Result<Value, MCPError>
where
    C: surrealdb::Connection,
{
    let params: GetChildrenParams = serde_json::from_value(params)
        .map_err(|e| MCPError::invalid_params(format!("Invalid parameters: {}", e)))?;

    // Get children in order
    let children =
        get_children_ordered(node_service, &params.parent_id, params.include_content).await?;

    let child_count = children.len();

    Ok(json!({
        "parent_id": params.parent_id,
        "child_count": child_count,
        "children": children
    }))
}

/// Handle insert_child_at_index MCP request
pub async fn handle_insert_child_at_index<C>(
    node_service: &Arc<NodeService<C>>,
    params: Value,
) -> Result<Value, MCPError>
where
    C: surrealdb::Connection,
{
    let params: InsertChildAtIndexParams = serde_json::from_value(params)
        .map_err(|e| MCPError::invalid_params(format!("Invalid parameters: {}", e)))?;

    // 1. Ensure parent exists (auto-create if date format)
    ensure_parent_exists(node_service, &params.parent_id).await?;

    // 2. Get parent to verify it exists (hierarchy managed via edges)
    let _parent = node_service
        .get_node(&params.parent_id)
        .await
        .map_err(|e| MCPError::internal_error(format!("Failed to get parent: {}", e)))?
        .ok_or_else(|| {
            MCPError::invalid_params(format!("Parent node '{}' not found", params.parent_id))
        })?;

    // 3. Get all siblings in order
    let children_info = get_children_ordered(node_service, &params.parent_id, false).await?;

    // 4. Calculate insert_after_node_id based on index
    // API semantics: insert_after_node_id = the sibling to insert AFTER
    // Note: The backend's calculate_sibling_position converts None to "append at end"
    // by finding the last child. We need to explicitly pass the correct value for ALL cases.
    //
    // - Index 0 on empty list → None (first position)
    // - Index 0 on non-empty list → Need to insert at beginning, but None would be transformed
    //   to "after last child". We need to use reorder after creation instead.
    // - Index N → insert_after_node_id = children[N-1] (insert after the previous child)
    // - Index >= length → insert_after_node_id = children.last() (append at end)
    //
    // For index 0 on non-empty list, we'll create at end then reorder to beginning.
    let needs_reorder_to_beginning = params.index == 0 && !children_info.is_empty();

    let insert_after_node_id = if children_info.is_empty() {
        None // Empty list - first position (beginning == end)
    } else if params.index >= children_info.len() || params.index == 0 {
        // Append at end (will reorder to beginning if index == 0)
        Some(children_info.last().unwrap().node_id.clone())
    } else {
        // Insert after the child at index-1
        Some(children_info[params.index - 1].node_id.clone())
    };

    // 5. Create node using pointer-based operation
    // Note: container/root is auto-derived from parent chain by backend
    let node_id = node_service
        .create_node_with_parent(crate::services::CreateNodeParams {
            id: None, // MCP generates IDs server-side
            node_type: params.node_type.clone(),
            content: params.content,
            parent_id: Some(params.parent_id.clone()),
            insert_after_node_id: insert_after_node_id.clone(),
            properties: params.properties,
        })
        .await
        .map_err(|e| MCPError::node_creation_failed(format!("Failed to create node: {}", e)))?;

    // 6. If index 0 was requested on non-empty list, reorder to beginning
    // We created at end, now move to beginning using reorder
    if needs_reorder_to_beginning {
        // Get the newly created node to get its version for OCC
        let created_node = node_service
            .get_node(&node_id)
            .await
            .map_err(|e| MCPError::internal_error(format!("Failed to get created node: {}", e)))?
            .ok_or_else(|| MCPError::internal_error("Created node not found".to_string()))?;

        // Reorder to beginning (insert_after = None means first position)
        node_service
            .reorder_node_with_occ(&node_id, created_node.version, None)
            .await
            .map_err(|e| {
                MCPError::internal_error(format!("Failed to reorder node to beginning: {}", e))
            })?;
    }

    // Note: With graph-native architecture (fractional ordering on has_child edges),
    // sibling chain fixing is no longer needed. The create_node operation with
    // insert_after_node_id already establishes the correct order.

    Ok(json!({
        "node_id": node_id,
        "parent_id": params.parent_id,
        "index": params.index,
        "node_type": params.node_type
    }))
}

/// Handle move_child_to_index MCP request
pub async fn handle_move_child_to_index<C>(
    node_service: &Arc<NodeService<C>>,
    params: Value,
) -> Result<Value, MCPError>
where
    C: surrealdb::Connection,
{
    let params: MoveChildToIndexParams = serde_json::from_value(params)
        .map_err(|e| MCPError::invalid_params(format!("Invalid parameters: {}", e)))?;

    // 1. Verify the node exists
    let _node = node_service
        .get_node(&params.node_id)
        .await
        .map_err(|e| MCPError::internal_error(format!("Failed to get node: {}", e)))?
        .ok_or_else(|| MCPError::invalid_params(format!("Node '{}' not found", params.node_id)))?;

    // Get parent using graph traversal
    let parent_id = node_service
        .get_parent(&params.node_id)
        .await
        .map_err(|e| MCPError::internal_error(format!("Failed to get parent: {}", e)))?
        .map(|n| n.id)
        .ok_or_else(|| {
            MCPError::invalid_params(format!(
                "Node '{}' has no parent (is a root node) - cannot reorder",
                params.node_id
            ))
        })?;

    // 2. Get all siblings in current order (excluding the node being moved)
    let all_children = get_children_ordered(node_service, &parent_id, false).await?;
    let siblings: Vec<_> = all_children
        .into_iter()
        .filter(|c| c.node_id != params.node_id)
        .collect();

    // 3. Calculate insert_after from target index
    // API semantics: insert_after = the sibling to insert AFTER
    // - Index 0 → insert_after = None (insert at beginning)
    // - Index N → insert_after = siblings[N-1] (insert after the (N-1)th sibling)
    // - Index >= siblings.len() → insert_after = last sibling (append at end)
    let insert_after = if params.index == 0 {
        None // Insert at beginning
    } else if params.index >= siblings.len() {
        // Append at end - insert after last sibling
        siblings.last().map(|s| s.node_id.clone())
    } else {
        // Insert after the sibling at index-1 (so node ends up at index)
        Some(siblings[params.index - 1].node_id.clone())
    };

    // 4. Use reorder_node operation (which handles edge ordering)
    node_service
        .reorder_node_with_occ(&params.node_id, params.version, insert_after.as_deref())
        .await
        .map_err(service_error_to_mcp)?;

    Ok(json!({
        "node_id": params.node_id,
        "new_index": params.index,
        "parent_id": parent_id
    }))
}

/// Handle get_child_at_index MCP request
pub async fn handle_get_child_at_index<C>(
    node_service: &Arc<NodeService<C>>,
    params: Value,
) -> Result<Value, MCPError>
where
    C: surrealdb::Connection,
{
    let params: GetChildAtIndexParams = serde_json::from_value(params)
        .map_err(|e| MCPError::invalid_params(format!("Invalid parameters: {}", e)))?;

    // Get all children in order
    let children =
        get_children_ordered(node_service, &params.parent_id, params.include_content).await?;

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
pub async fn handle_get_node_tree<C>(
    node_service: &Arc<NodeService<C>>,
    params: Value,
) -> Result<Value, MCPError>
where
    C: surrealdb::Connection,
{
    let params: GetNodeTreeParams = serde_json::from_value(params)
        .map_err(|e| MCPError::invalid_params(format!("Invalid parameters: {}", e)))?;

    // Validate max_depth
    if params.max_depth == 0 || params.max_depth > 100 {
        return Err(MCPError::invalid_params(format!(
            "max_depth must be between 1 and 100, got {}",
            params.max_depth
        )));
    }

    let root = node_service
        .get_node(&params.node_id)
        .await
        .map_err(|e| MCPError::internal_error(format!("Failed to get node: {}", e)))?
        .ok_or_else(|| MCPError::invalid_params(format!("Node '{}' not found", params.node_id)))?;

    let tree = build_tree_node(
        node_service,
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
/// ```ignore
/// let params = json!({
///     "node_ids": ["task-1", "task-2", "task-3"]
/// });
/// let result = handle_get_nodes_batch(&operations, params).await?;
/// // Returns all found nodes + list of IDs that don't exist
/// ```
pub async fn handle_get_nodes_batch<C>(
    node_service: &Arc<NodeService<C>>,
    params: Value,
) -> Result<Value, MCPError>
where
    C: surrealdb::Connection,
{
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

    // Fetch all nodes and convert to typed representations
    let mut nodes = Vec::new();
    let mut not_found = Vec::new();

    for node_id in params.node_ids {
        match node_service.get_node(&node_id).await {
            Ok(Some(node)) => {
                // Convert to strongly-typed JSON representation
                match node_to_typed_value(node) {
                    Ok(typed_value) => nodes.push(typed_value),
                    Err(e) => {
                        tracing::warn!("Error converting node {}: {}", node_id, e.message);
                        not_found.push(node_id);
                    }
                }
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
/// Schema validation is handled by NodeService::validate_node_against_schema(),
/// which validates enum values and required fields. No SchemaService needed.
///
/// # Example
///
/// ```ignore
/// let params = json!({
///     "updates": [
///         { "id": "task-1", "content": "- [x] Done" },
///         { "id": "task-2", "properties": { "priority": "high" } }
///     ]
/// });
/// let result = handle_update_nodes_batch(&node_service, params).await?;
/// ```
pub async fn handle_update_nodes_batch<C>(
    node_service: &Arc<NodeService<C>>,
    params: Value,
) -> Result<Value, MCPError>
where
    C: surrealdb::Connection,
{
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
                match node_service.get_node(&update.id).await {
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
                        let mcp_error = service_error_to_mcp(e);
                        failed.push(BatchUpdateFailure {
                            id: update.id,
                            error: mcp_error.message,
                        });
                        continue;
                    }
                }
            }
        };

        // Build NodeUpdate from batch update item
        // NodeService.update_node_with_occ validates against schema automatically
        // Embeddings are auto-generated via root-aggregate model (Issue #729)
        let node_update = NodeUpdate {
            content: update.content,
            node_type: update.node_type,
            properties: update.properties,
        };

        // Apply update via NodeService (enforces all business rules)
        match node_service
            .update_node_with_occ(&update.id, version, node_update)
            .await
        {
            Ok(_) => {
                updated.push(update.id);
            }
            Err(e) => {
                tracing::warn!("Failed to update node {}: {}", update.id, e);
                let mcp_error = service_error_to_mcp(e);
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
