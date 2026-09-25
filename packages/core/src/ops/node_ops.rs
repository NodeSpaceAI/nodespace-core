//! Node Operations
//!
//! Typed orchestration for node CRUD. Extracted from MCP handlers so both
//! MCP and local agent tools share the same logic.

use crate::models::{FilterOperator, Node, NodeFilter, NodeUpdate, OrderBy, PropertyFilter};
use crate::ops::OpsError;
use crate::services::{CollectionService, InsertPositionOwned, NodeService};
use serde::Deserialize;
use serde_json::Value;
use std::collections::HashSet;
use std::sync::Arc;

// ============================================================================
// Input / Output types
// ============================================================================

#[derive(Debug)]
pub struct CreateNodeInput {
    /// Optional explicit node ID; generated if `None`.
    pub id: Option<String>,
    pub node_type: String,
    pub content: String,
    pub parent_id: Option<String>,
    pub position: InsertPositionOwned,
    pub properties: Value,
    /// Collection paths to add the node to (e.g. "hr:policy:vacation").
    /// Missing path segments are created. Empty = no collection assignment.
    pub collections: Vec<String>,
    /// Collection IDs to add the node to. The ID-based counterpart of
    /// [`Self::collections`]; callers pass one form or the other, never both.
    pub collection_ids: Vec<String>,
    /// Optional lifecycle status. Only `"active"` and `"archived"` are supported;
    /// any other value is rejected at the storage boundary.
    pub lifecycle_status: Option<String>,
}

#[derive(Debug)]
pub struct CreateNodeOutput {
    pub node_id: String,
    pub node_type: String,
    pub parent_id: Option<String>,
    /// Leaf collection IDs the node was added to, in request order.
    pub collection_ids: Vec<String>,
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
    /// Collection paths to add the node to. Missing segments are created.
    pub add_to_collections: Vec<String>,
    /// Collection IDs to add the node to. The ID-based counterpart of
    /// [`Self::add_to_collections`]; callers pass one form or the other.
    pub add_to_collection_ids: Vec<String>,
    /// Collection IDs to remove the node from. IDs, not paths: removal detaches
    /// an existing `member_of` edge, so a path passed here would resolve to
    /// nothing and silently remove no membership.
    pub remove_from_collection_ids: Vec<String>,
    pub lifecycle_status: Option<String>,
}

#[derive(Debug)]
pub struct UpdateNodeOutput {
    pub node_id: String,
    pub version: i64,
    pub node_data: Value,
    /// Leaf collection IDs added, in request order.
    pub collections_added: Vec<String>,
    /// Collection IDs removed, in request order.
    pub collections_removed: Vec<String>,
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
    pub deleted_count: u64,
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
    let collection_paths = input.collections.clone();
    let collection_ids_requested = input.collection_ids.clone();
    let node_type = input.node_type.clone();

    if !collection_paths.is_empty() && !collection_ids_requested.is_empty() {
        return Err(OpsError::InvalidParams(
            "Pass collection paths or collection IDs, not both".to_string(),
        ));
    }

    // ADR-069 §5/F6: lifecycle_status is passed into the create itself
    // rather than applied via a separate update afterward. The old
    // two-write shape had a real gap — a failure on the second write left a
    // node the caller asked to be archived/draft live and visible as
    // active, while the caller was told the operation failed. Setting it on
    // the initial insert makes that window impossible: the status is part
    // of the one write that creates the row.
    let node_id = node_service
        .create_node_with_parent(crate::services::CreateNodeParams {
            id: input.id,
            node_type: input.node_type,
            content: input.content,
            parent_id: input.parent_id,
            position: input.position,
            properties: input.properties,
            lifecycle_status: input.lifecycle_status,
        })
        .await?;

    // Add to every requested collection. A failure propagates rather than
    // returning a success that hid an unfiled node — but it is not atomic with
    // the node write above: the node stays persisted, as do any collections
    // already joined before the failing one. The caller sees the error and can
    // retry the join; it does not get a silent partial success.
    let mut collection_ids = Vec::with_capacity(collection_paths.len());
    if !collection_paths.is_empty() || !collection_ids_requested.is_empty() {
        let collection_service = CollectionService::new(node_service.store(), node_service);
        for path in &collection_paths {
            let resolved = collection_service
                .add_to_collection_by_path(&node_id, path)
                .await?;
            collection_ids.push(resolved.leaf_id().to_string());
        }
        for collection_id in &collection_ids_requested {
            collection_service
                .add_to_collection(&node_id, collection_id)
                .await?;
            collection_ids.push(collection_id.clone());
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
        collection_ids,
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
    if !input.add_to_collections.is_empty() && !input.add_to_collection_ids.is_empty() {
        return Err(OpsError::InvalidParams(
            "Pass collection paths or collection IDs, not both".to_string(),
        ));
    }

    let update = NodeUpdate {
        content: input.content,
        node_type: input.node_type,
        properties: input.properties,
        title: None,
        lifecycle_status: input.lifecycle_status,
    };

    // When there are no field changes, skip the core update call — a no-op
    // NodeUpdate would be rejected with "Update contains no changes". Collection
    // operations below are still applied.
    let current_node = if update.is_empty() {
        node_service
            .get_node(&input.node_id)
            .await
            .map_err(|e| OpsError::Internal(format!("Failed to get node: {}", e)))?
            .ok_or_else(|| OpsError::NotFound {
                id: input.node_id.clone(),
            })?
    } else {
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

        match node_service
            .update_node(&input.node_id, version, update)
            .await
        {
            Ok(node) => node,
            Err(crate::services::NodeServiceError::VersionConflict {
                node_id,
                expected_version,
                actual_version,
            }) => {
                // Embed current state for client-side merge, in the FLATTENED
                // wire shape (`node_to_typed_value`) every other read/write
                // returns. Serializing the raw `Node` here instead would leave
                // type-specific fields buried under `properties[<type>]`, and
                // the client hydrates this payload directly into its store —
                // so an ai-chat conflict would strand the viewer with
                // `status`/`messages` undefined at the top level.
                let current_node = node_service
                    .get_node(&node_id)
                    .await
                    .ok()
                    .flatten()
                    .and_then(|n| node_to_typed_value(n).ok());
                return Err(OpsError::VersionConflict {
                    node_id,
                    expected: expected_version,
                    actual: actual_version,
                    current_node,
                });
            }
            Err(e) => return Err(OpsError::from(e)),
        }
    };

    // Handle collection operations
    let collection_service = CollectionService::new(node_service.store(), node_service);
    let mut collections_added =
        Vec::with_capacity(input.add_to_collections.len() + input.add_to_collection_ids.len());
    let mut collections_removed = Vec::with_capacity(input.remove_from_collection_ids.len());

    for path in &input.add_to_collections {
        let resolved = collection_service
            .add_to_collection_by_path(&input.node_id, path)
            .await?;
        collections_added.push(resolved.leaf_id().to_string());
    }

    for collection_id in &input.add_to_collection_ids {
        collection_service
            .add_to_collection(&input.node_id, collection_id)
            .await?;
        collections_added.push(collection_id.clone());
    }

    for collection_id in &input.remove_from_collection_ids {
        collection_service
            .remove_from_collection(&input.node_id, collection_id)
            .await?;
        collections_removed.push(collection_id.clone());
    }

    // Re-fetch if collection membership changed
    let final_node = if !collections_added.is_empty() || !collections_removed.is_empty() {
        node_service
            .get_node(&input.node_id)
            .await
            .map_err(|e| OpsError::Internal(format!("Failed to fetch updated node: {}", e)))?
            .unwrap_or(current_node)
    } else {
        current_node
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
        collections_added,
        collections_removed,
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

    let result = node_service.delete_node(&input.node_id, version).await?;

    Ok(DeleteNodeOutput {
        node_id: input.node_id,
        existed: result.existed,
        deleted_count: result.deleted_count,
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

    // pagination differs for a collection-scoped query. Pushing offset/limit
    // to SQL paginated the WRONG set (membership is filtered after the query), and
    // the old fixed 1000 over-fetch silently dropped any member outside the newest
    // 1000 rows. Instead SCOPE the SQL to the collection's members via `with_ids` —
    // which `NodeQuery.ids` now translates to a chunked `id IN (…)` (bounded by the
    // member set, NOT a full-table scan) — and apply offset/limit IN MEMORY below.
    match &collection_member_ids {
        Some(member_ids) if member_ids.is_empty() => {
            // Empty collection → no members can match; skip the query entirely.
            return Ok(QueryNodesOutput {
                nodes: vec![],
                count: 0,
                collection_id,
            });
        }
        Some(member_ids) => {
            filter = filter.with_ids(member_ids.iter().cloned().collect());
        }
        None => {
            filter = filter.with_limit(input.limit.unwrap_or(100));
            if let Some(offset) = input.offset {
                filter = filter.with_offset(offset);
            }
        }
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
                    let prop_filter = PropertyFilter::new(path, operator, f.value.clone())
                        .map_err(|e| {
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

    // Paginate IN MEMORY. The SQL is already id-scoped to members (the
    // `id IN (…)` from `with_ids`), so this `member_ids.contains` is a genuine
    // defensive double-check, not the thing producing correctness. Apply offset +
    // limit over the membership set (CreatedDesc-ordered) so `offset>0` and `limit`
    // page the actual members, not the global set.
    let filtered_nodes = if let Some(member_ids) = collection_member_ids {
        let mut result: Vec<_> = nodes
            .into_iter()
            .filter(|n| member_ids.contains(&n.id))
            .collect();
        let offset = input.offset.unwrap_or(0);
        if offset > 0 {
            result = result.into_iter().skip(offset).collect();
        }
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
        assert!(matches!(
            parse_filter_operator("equals"),
            Ok(FilterOperator::Equals)
        ));
        assert!(matches!(
            parse_filter_operator("not_equals"),
            Ok(FilterOperator::NotEquals)
        ));
        assert!(matches!(
            parse_filter_operator("contains"),
            Ok(FilterOperator::Contains)
        ));
        assert!(matches!(
            parse_filter_operator("starts_with"),
            Ok(FilterOperator::StartsWith)
        ));
        assert!(matches!(
            parse_filter_operator("ends_with"),
            Ok(FilterOperator::EndsWith)
        ));
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

    /// `create_node`'s `NodeServiceError` -> `OpsError` conversion must go
    /// through the shared `From` impl (`?`), not a hand-rolled `map_err` that
    /// flattens every failure to `Internal`. The daemon's gRPC boundary keys
    /// its status code off the `OpsError` variant
    /// (`OpsError::InvalidParams` -> `INVALID_ARGUMENT`,
    /// `OpsError::Internal` -> `INTERNAL`), so losing the variant here would
    /// report a caller mistake (an unknown node_type) as a server fault.
    #[tokio::test]
    async fn create_node_unknown_type_surfaces_as_invalid_params_not_internal() {
        let temp_dir = std::env::temp_dir().join(format!(
            "test_create_node_ops_error_{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&temp_dir).unwrap();
        let store = crate::db::SqliteStore::new(temp_dir.join("test.db"))
            .await
            .unwrap();
        let mut store = std::sync::Arc::new(store);
        let ns = std::sync::Arc::new(crate::services::NodeService::new(&mut store).await.unwrap());

        let err = create_node(
            &ns,
            CreateNodeInput {
                id: None,
                node_type: "Not A Real Type".to_string(),
                content: "content".to_string(),
                parent_id: None,
                position: crate::services::InsertPositionOwned::End,
                properties: serde_json::json!({}),
                collections: Vec::new(),
                collection_ids: Vec::new(),
                lifecycle_status: None,
            },
        )
        .await
        .expect_err("an unknown node_type must be rejected");

        assert!(
            matches!(err, OpsError::InvalidParams(_)),
            "expected InvalidParams so the daemon reports INVALID_ARGUMENT, got: {:?}",
            err
        );
    }
}
