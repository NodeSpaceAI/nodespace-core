//! Batch node operations used by performance benchmarks.
//!
//! These were previously MCP handlers (`handle_get_nodes_batch` /
//! `handle_update_nodes_batch`) reached only through the deleted JSON-RPC
//! transport. They survive here because `core/benches/performance.rs` exercises
//! them as the canonical batch read/write path. They take and return
//! `serde_json::Value` to match the benchmark call sites.

use crate::models::{node_to_typed_value, NodeUpdate};
use crate::services::NodeService;
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;

/// Parameters for [`get_nodes_batch`].
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GetNodesBatchParams {
    pub node_ids: Vec<String>,
}

/// A single update in a batch request.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BatchUpdateItem {
    /// Node ID to update.
    pub id: String,
    /// Expected version for optimistic concurrency control. If not provided,
    /// the current version is fetched (optimistic: assumes no conflict).
    #[serde(default)]
    pub version: Option<i64>,
    /// Optional updated content.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    /// Optional updated node type.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub node_type: Option<String>,
    /// Optional updated properties.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub properties: Option<Value>,
}

/// Parameters for [`update_nodes_batch`].
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateNodesBatchParams {
    pub updates: Vec<BatchUpdateItem>,
}

/// Failed update info.
#[derive(Debug, serde::Serialize)]
struct BatchUpdateFailure {
    id: String,
    error: String,
}

/// Fetch multiple nodes in a single request.
///
/// More efficient than calling `get_node` repeatedly when many nodes are
/// needed at once. Returns the found nodes (as typed JSON), the IDs that
/// weren't found, and a count.
pub async fn get_nodes_batch(
    node_service: &Arc<NodeService>,
    params: Value,
) -> Result<Value, String> {
    let params: GetNodesBatchParams =
        serde_json::from_value(params).map_err(|e| format!("Invalid parameters: {e}"))?;

    if params.node_ids.is_empty() {
        return Err("node_ids cannot be empty".to_string());
    }

    if params.node_ids.len() > 100 {
        return Err(format!(
            "Batch size exceeds maximum of 100 nodes (got {} nodes)",
            params.node_ids.len()
        ));
    }

    let mut nodes = Vec::new();
    let mut not_found = Vec::new();

    for node_id in params.node_ids {
        match node_service.get_node(&node_id).await {
            Ok(Some(node)) => match node_to_typed_value(node) {
                Ok(typed_value) => nodes.push(typed_value),
                Err(e) => {
                    tracing::warn!("Error converting node {}: {}", node_id, e);
                    not_found.push(node_id);
                }
            },
            Ok(None) => not_found.push(node_id),
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

/// Update multiple nodes in a single request.
///
/// Applies updates sequentially via [`NodeService::update_node`], which enforces
/// schema validation and business rules. Returns both successful and failed
/// update IDs for client handling.
pub async fn update_nodes_batch(
    node_service: &Arc<NodeService>,
    params: Value,
) -> Result<Value, String> {
    let params: UpdateNodesBatchParams =
        serde_json::from_value(params).map_err(|e| format!("Invalid parameters: {e}"))?;

    if params.updates.is_empty() {
        return Err("updates cannot be empty".to_string());
    }

    if params.updates.len() > 100 {
        return Err(format!(
            "Batch size exceeds maximum of 100 updates (got {} updates)",
            params.updates.len()
        ));
    }

    let mut updated = Vec::new();
    let mut failed: Vec<BatchUpdateFailure> = Vec::new();

    for update in params.updates {
        // If version not provided, fetch current version (optimistic: assumes no
        // concurrent updates). ⚠️ This bypasses optimistic concurrency control.
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
                        failed.push(BatchUpdateFailure {
                            id: update.id,
                            error: e.to_string(),
                        });
                        continue;
                    }
                }
            }
        };

        let node_update = NodeUpdate {
            content: update.content,
            node_type: update.node_type,
            properties: update.properties,
            title: None,
            lifecycle_status: None,
            ..Default::default()
        };

        match node_service
            .update_node(&update.id, version, node_update)
            .await
        {
            Ok(_) => updated.push(update.id),
            Err(e) => {
                tracing::warn!("Failed to update node {}: {}", update.id, e);
                failed.push(BatchUpdateFailure {
                    id: update.id,
                    error: e.to_string(),
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
