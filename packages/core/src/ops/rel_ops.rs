//! Relationship Operations
//!
//! Typed orchestration for relationship CRUD. Extracted from MCP handlers.

use crate::ops::OpsError;
use crate::services::NodeService;
use serde_json::{json, Value};
use std::sync::Arc;

// ============================================================================
// Input / Output types
// ============================================================================

#[derive(Debug)]
pub struct CreateRelInput {
    pub source_id: String,
    pub relationship_name: String,
    pub target_id: String,
    pub edge_data: Option<Value>,
}

#[derive(Debug)]
pub struct CreateRelOutput {
    pub source_id: String,
    pub relationship_name: String,
    pub target_id: String,
}

#[derive(Debug)]
pub struct DeleteRelInput {
    pub source_id: String,
    pub relationship_name: String,
    pub target_id: String,
}

#[derive(Debug)]
pub struct GetRelatedInput {
    pub node_id: String,
    pub relationship_name: String,
    /// "out" (forward) or "in" (reverse)
    pub direction: String,
}

#[derive(Debug)]
pub struct GetRelatedOutput {
    pub node_id: String,
    pub relationship_name: String,
    pub direction: String,
    pub related_nodes: Vec<Value>,
    pub count: usize,
}

// ============================================================================
// Operations
// ============================================================================

/// Create a relationship edge between two nodes.
pub async fn create_relationship(
    node_service: &Arc<NodeService>,
    input: CreateRelInput,
) -> Result<CreateRelOutput, OpsError> {
    let edge_data = input.edge_data.unwrap_or(json!({}));

    node_service
        .create_relationship(
            &input.source_id,
            &input.relationship_name,
            &input.target_id,
            edge_data,
        )
        .await
        .map_err(|e| OpsError::Internal(format!("Failed to create relationship: {}", e)))?;

    Ok(CreateRelOutput {
        source_id: input.source_id,
        relationship_name: input.relationship_name,
        target_id: input.target_id,
    })
}

/// Delete a relationship edge. Idempotent.
pub async fn delete_relationship(
    node_service: &Arc<NodeService>,
    input: DeleteRelInput,
) -> Result<(), OpsError> {
    node_service
        .delete_relationship(&input.source_id, &input.relationship_name, &input.target_id)
        .await
        .map_err(|e| OpsError::Internal(format!("Failed to delete relationship: {}", e)))?;

    Ok(())
}

/// Get nodes related via a specific relationship.
pub async fn get_related_nodes(
    node_service: &Arc<NodeService>,
    input: GetRelatedInput,
) -> Result<GetRelatedOutput, OpsError> {
    let nodes = node_service
        .get_related_nodes(&input.node_id, &input.relationship_name, &input.direction)
        .await
        .map_err(|e| OpsError::Internal(format!("Failed to get related nodes: {}", e)))?;

    let count = nodes.len();
    let related_nodes: Vec<Value> = nodes
        .into_iter()
        .map(|n| serde_json::to_value(n).unwrap_or(json!(null)))
        .collect();

    Ok(GetRelatedOutput {
        node_id: input.node_id,
        relationship_name: input.relationship_name,
        direction: input.direction,
        related_nodes,
        count,
    })
}
