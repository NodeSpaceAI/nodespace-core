//! Node Operations
//!
//! Typed orchestration for node CRUD. Extracted from MCP handlers so both
//! MCP and local agent tools share the same logic.

use crate::models::{
    FilterOperator, Node, NodeFilter, NodeUpdate, OrderBy, PropertyFilter,
};
use crate::ops::OpsError;
use crate::services::{CollectionService, NodeService};
use serde::Deserialize;
use serde_json::Value;
use std::collections::HashSet;
use std::sync::Arc;

// ============================================================================
// Input / Output types
// ============================================================================

#[derive(Debug)]
pub struct CreateNodeInput {
    pub node_type: String,
    pub content: String,
    pub parent_id: Option<String>,
    pub properties: Value,
    /// Optional collection path (e.g. "hr:policy:vacation")
    pub collection: Option<String>,
    /// Optional lifecycle status ("active", "archived", "deleted")
    pub lifecycle_status: Option<String>,
}

#[derive(Debug)]
pub struct CreateNodeOutput {
    pub node_id: String,
    pub node_type: String,
    pub parent_id: Option<String>,
    pub collection_id: Option<String>,
    pub node_data: Value,
}

#[derive(Debug)]
pub struct GetNodeInput {
    pub node_id: String,
}

pub type GetNodeOutput = Value;

#[derive(Debug)]
pub struct UpdateNodeInput {
    pub node_id: String,
    /// If None, current version is auto-fetched (convenient for agents).
    pub version: Option<i64>,
    pub node_type: Option<String>,
    pub content: Option<String>,
    pub properties: Option<Value>,
    pub add_to_collection: Option<String>,
    pub remove_from_collection: Option<String>,
    pub lifecycle_status: Option<String>,
}

#[derive(Debug)]
pub struct UpdateNodeOutput {
    pub node_id: String,
    pub version: i64,
    pub node_data: Value,
    pub collection_added: Option<String>,
    pub collection_removed: Option<String>,
}

#[derive(Debug)]
pub struct DeleteNodeInput {
    pub node_id: String,
    pub version: Option<i64>,
}

#[derive(Debug)]
pub struct DeleteNodeOutput {
    pub node_id: String,
    pub existed: bool,
}

/// A single filter condition
#[derive(Debug, Deserialize)]
pub struct QueryFilterItem {
    pub field: String,
    pub operator: String,
    pub value: Value,
}

#[derive(Debug)]
pub struct QueryNodesInput {
    pub node_type: Option<String>,
    pub parent_id: Option<String>,
    pub root_id: Option<String>,
    pub limit: Option<usize>,
    pub offset: Option<usize>,
    pub collection_id: Option<String>,
    pub collection: Option<String>,
    pub filters: Option<Vec<QueryFilterItem>>,
}

#[derive(Debug)]
pub struct QueryNodesOutput {
    pub nodes: Vec<Value>,
    pub count: usize,
    pub collection_id: Option<String>,
}

// ============================================================================
// Helpers
// ============================================================================

fn node_to_typed_value(node: Node) -> Result<Value, OpsError> {
    crate::models::node_to_typed_value(node).map_err(OpsError::Internal)
}

fn nodes_to_typed_values(nodes: Vec<Node>) -> Result<Vec<Value>, OpsError> {
    crate::models::nodes_to_typed_values(nodes).map_err(OpsError::Internal)
}

fn parse_filter_operator(op: &str) -> Result<FilterOperator, OpsError> {
    match op {
        "equals" => Ok(FilterOperator::Equals),
        "not_equals" => Ok(FilterOperator::NotEquals),
        "contains" => Ok(FilterOperator::Contains),
        "starts_with" => Ok(FilterOperator::StartsWith),
        "ends_with" => Ok(FilterOperator::EndsWith),
        other => Err(OpsError::InvalidParams(format!(
            "Unsupported filter operator: '{}'. Supported: equals, not_equals, contains, starts_with, ends_with",
            other
        ))),
    }
}

// ============================================================================
// Operations
// ============================================================================

/// Create a node, optionally adding to a collection and setting lifecycle status.
pub async fn create_node(
    node_service: &Arc<NodeService>,
    input: CreateNodeInput,
) -> Result<CreateNodeOutput, OpsError> {
    let parent_id = input.parent_id.clone();
    let collection_path = input.collection.clone();
    let node_type = input.node_type.clone();

    let node_id = node_service
        .create_node_with_parent(crate::services::CreateNodeParams {
            id: None,
            node_type: input.node_type,
            content: input.content,
            parent_id: input.parent_id,
            insert_after_node_id: None,
            properties: input.properties,
        })
        .await
        .map_err(|e| OpsError::Internal(format!("Failed to create node: {}", e)))?;

    // Add to collection if specified
    let collection_id = if let Some(path) = &collection_path {
        let collection_service = CollectionService::new(node_service.store(), node_service);
        let resolved = collection_service
            .add_to_collection_by_path(&node_id, path)
            .await?;
        Some(resolved.leaf_id().to_string())
    } else {
        None
    };

    // Apply non-default lifecycle status
    if let Some(ref lifecycle_status) = input.lifecycle_status {
        if lifecycle_status != "active" {
            let current_node = node_service
                .get_node(&node_id)
                .await
                .map_err(|e| OpsError::Internal(format!("Failed to get node: {}", e)))?
                .ok_or_else(|| OpsError::Internal("Created node not found".to_string()))?;

            let update = NodeUpdate {
                lifecycle_status: Some(lifecycle_status.clone()),
                ..Default::default()
            };

            node_service
                .update_node(&node_id, current_node.version, update)
                .await?;
        }
    }

    // Re-fetch for final state
    let created_node = node_service
        .get_node(&node_id)
        .await
        .map_err(|e| OpsError::Internal(format!("Failed to fetch created node: {}", e)))?
        .ok_or_else(|| OpsError::Internal("Created node not found".to_string()))?;

    let node_data = node_to_typed_value(created_node)?;

    Ok(CreateNodeOutput {
        node_id,
        node_type,
        parent_id,
        collection_id,
        node_data,
    })
}

/// Get a single node by ID.
pub async fn get_node(
    node_service: &Arc<NodeService>,
    input: GetNodeInput,
) -> Result<GetNodeOutput, OpsError> {
    let node = node_service
        .get_node(&input.node_id)
        .await
        .map_err(|e| OpsError::Internal(format!("Failed to get node: {}", e)))?
        .ok_or_else(|| OpsError::NotFound {
            id: input.node_id.clone(),
        })?;

    node_to_typed_value(node)
}

/// Update a node with auto-fetch of version when not provided.
/// On VersionConflict, embeds current node state in the error.
pub async fn update_node(
    node_service: &Arc<NodeService>,
    input: UpdateNodeInput,
) -> Result<UpdateNodeOutput, OpsError> {
    let update = NodeUpdate {
        content: input.content,
        node_type: input.node_type,
        properties: input.properties,
        title: None,
        lifecycle_status: input.lifecycle_status,
    };

    // Auto-fetch version if not provided
    let version = match input.version {
        Some(v) => v,
        None => {
            let node = node_service
                .get_node(&input.node_id)
                .await
                .map_err(|e| OpsError::Internal(format!("Failed to get node: {}", e)))?
                .ok_or_else(|| OpsError::NotFound {
                    id: input.node_id.clone(),
                })?;
            node.version
        }
    };

    let updated_node = match node_service
        .update_node(&input.node_id, version, update)
        .await
    {
        Ok(node) => node,
        Err(crate::services::NodeServiceError::VersionConflict {
            node_id,
            expected_version,
            actual_version,
        }) => {
            // Embed current state for client-side merge
            let current_node = node_service
                .get_node(&node_id)
                .await
                .ok()
                .flatten()
                .and_then(|n| serde_json::to_value(&n).ok());
            return Err(OpsError::VersionConflict {
                node_id,
                expected: expected_version,
                actual: actual_version,
                current_node,
            });
        }
        Err(e) => return Err(OpsError::from(e)),
    };

    // Handle collection operations
    let collection_service = CollectionService::new(node_service.store(), node_service);
    let mut collection_added = None;
    let mut collection_removed = None;

    if let Some(path) = &input.add_to_collection {
        let resolved = collection_service
            .add_to_collection_by_path(&input.node_id, path)
            .await?;
        collection_added = Some(resolved.leaf_id().to_string());
    }

    if let Some(collection_id) = &input.remove_from_collection {
        collection_service
            .remove_from_collection(&input.node_id, collection_id)
            .await?;
        collection_removed = Some(collection_id.clone());
    }

    // Re-fetch if collection membership changed
    let final_node = if input.add_to_collection.is_some() || input.remove_from_collection.is_some()
    {
        node_service
            .get_node(&input.node_id)
            .await
            .map_err(|e| OpsError::Internal(format!("Failed to fetch updated node: {}", e)))?
            .unwrap_or(updated_node)
    } else {
        updated_node
    };

    let node_data = node_to_typed_value(final_node)?;
    let version = node_data
        .get("version")
        .and_then(|v| v.as_i64())
        .unwrap_or(0);

    Ok(UpdateNodeOutput {
        node_id: input.node_id,
        version,
        node_data,
        collection_added,
        collection_removed,
    })
}

/// Delete a node with optional version check.
pub async fn delete_node(
    node_service: &Arc<NodeService>,
    input: DeleteNodeInput,
) -> Result<DeleteNodeOutput, OpsError> {
    // Auto-fetch version if not provided
    let version = match input.version {
        Some(v) => v,
        None => {
            let node = node_service
                .get_node(&input.node_id)
                .await
                .map_err(|e| OpsError::Internal(format!("Failed to get node: {}", e)))?
                .ok_or_else(|| OpsError::NotFound {
                    id: input.node_id.clone(),
                })?;
            node.version
        }
    };

    let result = node_service
        .delete_node(&input.node_id, version)
        .await?;

    Ok(DeleteNodeOutput {
        node_id: input.node_id,
        existed: result.existed,
    })
}

/// Query nodes with collection resolution, over-fetching, and post-filtering.
pub async fn query_nodes(
    node_service: &Arc<NodeService>,
    input: QueryNodesInput,
) -> Result<QueryNodesOutput, OpsError> {
    // Resolve collection ID if path provided
    let collection_id = if let Some(path) = &input.collection {
        let collection_service = CollectionService::new(node_service.store(), node_service);
        match collection_service.resolve_path(path).await {
            Ok(resolved) => Some(resolved.leaf_id().to_string()),
            Err(crate::services::NodeServiceError::CollectionNotFound(_)) => {
                return Ok(QueryNodesOutput {
                    nodes: vec![],
                    count: 0,
                    collection_id: None,
                });
            }
            Err(e) => return Err(OpsError::from(e)),
        }
    } else {
        input.collection_id.clone()
    };

    // Get collection members if filtering
    let collection_member_ids: Option<HashSet<String>> = if let Some(coll_id) = &collection_id {
        let collection_service = CollectionService::new(node_service.store(), node_service);
        let members = collection_service.get_collection_members(coll_id).await?;
        Some(members.into_iter().map(|n| n.id).collect())
    } else {
        None
    };

    // Build filter
    let mut filter = NodeFilter::new();

    if let Some(node_type) = input.node_type {
        filter = filter.with_node_type(node_type);
    }

    if input.parent_id.is_some() {
        tracing::warn!("parent_id filter ignored - use graph queries for relationship traversal");
    }
    if input.root_id.is_some() {
        tracing::warn!("root_id filter is deprecated - use graph queries for relationship traversal");
    }

    // Over-fetch when collection filtering
    let effective_limit = if collection_member_ids.is_some() {
        input.limit.map(|l| l * 3).unwrap_or(1000)
    } else {
        input.limit.unwrap_or(100)
    };
    filter = filter.with_limit(effective_limit);

    if let Some(offset) = input.offset {
        filter = filter.with_offset(offset);
    }

    // Apply structured filters
    if let Some(filters) = input.filters {
        let mut seen_fields = HashSet::new();
        for f in filters {
            if !seen_fields.insert(f.field.clone()) {
                return Err(OpsError::InvalidParams(format!(
                    "Duplicate filter field '{}'. Each field may appear at most once.",
                    f.field
                )));
            }
            let value_str = match &f.value {
                Value::String(s) => s.clone(),
                other => other.to_string(),
            };
            match (f.field.as_str(), f.operator.as_str()) {
                ("content", "contains") => {
                    filter = filter.with_content_contains(value_str);
                }
                ("title", "contains") => {
                    filter = filter.with_title_contains(value_str);
                }
                ("content" | "title", op) => {
                    return Err(OpsError::InvalidParams(format!(
                        "Field '{}' only supports 'contains' operator, got '{}'",
                        f.field, op
                    )));
                }
                (_field, op) => {
                    let operator = parse_filter_operator(op)?;
                    let path = format!("$.{}", f.field);
                    let prop_filter =
                        PropertyFilter::new(path, operator, f.value.clone()).map_err(|e| {
                            OpsError::InvalidParams(format!("Invalid property filter: {}", e))
                        })?;
                    filter = filter.with_property_filter(prop_filter);
                }
            }
        }
    }

    filter = filter.with_order_by(OrderBy::CreatedDesc);

    let nodes = node_service
        .query_nodes(filter)
        .await
        .map_err(|e| OpsError::Internal(format!("Failed to query nodes: {}", e)))?;

    // Post-filter by collection membership
    let filtered_nodes = if let Some(member_ids) = collection_member_ids {
        let mut result: Vec<_> = nodes
            .into_iter()
            .filter(|n| member_ids.contains(&n.id))
            .collect();
        if let Some(limit) = input.limit {
            result.truncate(limit);
        }
        result
    } else {
        nodes
    };

    let count = filtered_nodes.len();
    let typed_nodes = nodes_to_typed_values(filtered_nodes)?;

    Ok(QueryNodesOutput {
        nodes: typed_nodes,
        count,
        collection_id,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::NodeServiceError;

    #[test]
    fn test_parse_filter_operator_valid() {
        assert!(matches!(parse_filter_operator("equals"), Ok(FilterOperator::Equals)));
        assert!(matches!(parse_filter_operator("not_equals"), Ok(FilterOperator::NotEquals)));
        assert!(matches!(parse_filter_operator("contains"), Ok(FilterOperator::Contains)));
        assert!(matches!(parse_filter_operator("starts_with"), Ok(FilterOperator::StartsWith)));
        assert!(matches!(parse_filter_operator("ends_with"), Ok(FilterOperator::EndsWith)));
    }

    #[test]
    fn test_parse_filter_operator_invalid() {
        let err = parse_filter_operator("like").unwrap_err();
        assert!(matches!(err, OpsError::InvalidParams(_)));
    }

    #[test]
    fn test_ops_error_from_node_not_found() {
        let svc_err = NodeServiceError::NodeNotFound {
            id: "abc".to_string(),
        };
        let ops_err: OpsError = svc_err.into();
        assert!(matches!(ops_err, OpsError::NotFound { id } if id == "abc"));
    }

    #[test]
    fn test_ops_error_from_version_conflict() {
        let svc_err = NodeServiceError::VersionConflict {
            node_id: "n1".to_string(),
            expected_version: 5,
            actual_version: 3,
        };
        let ops_err: OpsError = svc_err.into();
        assert!(matches!(
            ops_err,
            OpsError::VersionConflict {
                node_id,
                expected: 5,
                actual: 3,
                ..
            } if node_id == "n1"
        ));
    }
}
