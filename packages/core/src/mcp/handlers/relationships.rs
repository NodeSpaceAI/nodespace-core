//! Relationship MCP Handlers
//!
//! Provides MCP handlers for relationship CRUD operations and NLP discovery.
//! These tools allow AI assistants to create, query, and manage relationships
//! between nodes based on schema-defined relationship types.
//!
//! ## Available Tools
//!
//! - `create_relationship` - Create a relationship edge between two nodes
//! - `delete_relationship` - Remove a relationship edge
//! - `get_related_nodes` - Query nodes connected via a relationship
//! - `get_relationship_graph` - Get the complete relationship graph for NLP
//! - `get_inbound_relationships` - Discover relationships pointing TO a type
//!
//! ## Architecture
//!
//! These handlers wrap the NodeService relationship CRUD API (Phase 4) and
//! NLP Discovery API (Phase 5) for MCP-compliant access.
//!
//! ## Wire Format (JSON Serialization)
//!
//! All response structs use `#[serde(rename_all = "camelCase")]` which means
//! Rust snake_case field names are serialized to camelCase in JSON responses.
//! For example:
//! - `source_id` → `"sourceId"`
//! - `relationship_name` → `"relationshipName"`
//! - `target_type` → `"targetType"`
//!
//! This follows MCP and JavaScript conventions for wire format.

use crate::mcp::types::MCPError;
use crate::services::NodeService;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::Arc;

// ============================================================================
// Request/Response Types
// ============================================================================

/// Parameters for create_relationship
#[derive(Debug, Deserialize)]
pub struct CreateRelationshipParams {
    /// ID of the source node
    pub source_id: String,
    /// Name of the relationship (must be defined in source node's schema)
    pub relationship_name: String,
    /// ID of the target node
    pub target_id: String,
    /// Optional edge data (JSON object with edge field values)
    #[serde(default)]
    pub edge_data: Option<Value>,
}

/// Parameters for delete_relationship
#[derive(Debug, Deserialize)]
pub struct DeleteRelationshipParams {
    /// ID of the source node
    pub source_id: String,
    /// Name of the relationship
    pub relationship_name: String,
    /// ID of the target node
    pub target_id: String,
}

/// Parameters for get_related_nodes
#[derive(Debug, Deserialize)]
pub struct GetRelatedNodesParams {
    /// ID of the node to get relationships for
    pub node_id: String,
    /// Name of the relationship
    pub relationship_name: String,
    /// Direction: "out" (forward) or "in" (reverse)
    #[serde(default = "default_direction")]
    pub direction: String,
}

fn default_direction() -> String {
    "out".to_string()
}

/// Parameters for get_inbound_relationships
#[derive(Debug, Deserialize)]
pub struct GetInboundRelationshipsParams {
    /// The node type to find inbound relationships for
    pub target_type: String,
}

/// Output for relationship creation
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateRelationshipOutput {
    pub success: bool,
    pub source_id: String,
    pub relationship_name: String,
    pub target_id: String,
}

/// Output for get_related_nodes
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GetRelatedNodesOutput {
    pub node_id: String,
    pub relationship_name: String,
    pub direction: String,
    pub related_nodes: Vec<Value>,
    pub count: usize,
}

/// Output for relationship graph
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RelationshipGraphOutput {
    /// List of edges: (source_type, relationship_name, target_type)
    pub edges: Vec<RelationshipEdge>,
    pub total_edges: usize,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RelationshipEdge {
    pub source_type: String,
    pub relationship_name: String,
    pub target_type: String,
}

/// Output for inbound relationships
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InboundRelationshipsOutput {
    pub target_type: String,
    pub inbound_relationships: Vec<InboundRelationshipInfo>,
    pub count: usize,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InboundRelationshipInfo {
    pub source_type: String,
    pub relationship_name: String,
    pub reverse_name: Option<String>,
    pub cardinality: String,
    pub edge_table: String,
}

// ============================================================================
// Handler Implementations
// ============================================================================

/// Create a relationship between two nodes
///
/// # MCP Tool: create_relationship
///
/// Creates an edge between source and target nodes using the relationship
/// defined in the source node's schema.
///
/// ## Parameters
/// - `source_id` - ID of the source node
/// - `relationship_name` - Name of the relationship from the schema
/// - `target_id` - ID of the target node
/// - `edge_data` - Optional JSON object with edge field values
///
/// ## Example
/// ```json
/// {
///   "source_id": "invoice-001",
///   "relationship_name": "billed_to",
///   "target_id": "customer-acme",
///   "edge_data": {"billing_date": "2025-01-15"}
/// }
/// ```
pub async fn handle_create_relationship(
    node_service: &Arc<NodeService>,
    params: Value,
) -> Result<Value, MCPError> {
    let params: CreateRelationshipParams = serde_json::from_value(params)
        .map_err(|e| MCPError::invalid_params(format!("Invalid parameters: {}", e)))?;

    let edge_data = params.edge_data.unwrap_or(json!({}));

    node_service
        .create_relationship(
            &params.source_id,
            &params.relationship_name,
            &params.target_id,
            edge_data,
        )
        .await
        .map_err(|e| MCPError::internal_error(format!("Failed to create relationship: {}", e)))?;

    Ok(serde_json::to_value(CreateRelationshipOutput {
        success: true,
        source_id: params.source_id,
        relationship_name: params.relationship_name,
        target_id: params.target_id,
    })
    .unwrap())
}

/// Delete a relationship between two nodes
///
/// # MCP Tool: delete_relationship
///
/// Removes the edge between source and target nodes. This is idempotent -
/// succeeds even if the edge doesn't exist.
///
/// ## Parameters
/// - `source_id` - ID of the source node
/// - `relationship_name` - Name of the relationship
/// - `target_id` - ID of the target node
pub async fn handle_delete_relationship(
    node_service: &Arc<NodeService>,
    params: Value,
) -> Result<Value, MCPError> {
    let params: DeleteRelationshipParams = serde_json::from_value(params)
        .map_err(|e| MCPError::invalid_params(format!("Invalid parameters: {}", e)))?;

    node_service
        .delete_relationship(
            &params.source_id,
            &params.relationship_name,
            &params.target_id,
        )
        .await
        .map_err(|e| MCPError::internal_error(format!("Failed to delete relationship: {}", e)))?;

    Ok(json!({
        "success": true,
        "message": "Relationship deleted"
    }))
}

/// Get nodes related via a specific relationship
///
/// # MCP Tool: get_related_nodes
///
/// Queries the edge table and returns all nodes connected via the specified
/// relationship. Supports both forward ("out") and reverse ("in") directions.
///
/// ## Parameters
/// - `node_id` - ID of the node to get relationships for
/// - `relationship_name` - Name of the relationship
/// - `direction` - "out" for forward, "in" for reverse (default: "out")
///
/// ## Example
/// ```json
/// {
///   "node_id": "invoice-001",
///   "relationship_name": "billed_to",
///   "direction": "out"
/// }
/// ```
pub async fn handle_get_related_nodes(
    node_service: &Arc<NodeService>,
    params: Value,
) -> Result<Value, MCPError> {
    let params: GetRelatedNodesParams = serde_json::from_value(params)
        .map_err(|e| MCPError::invalid_params(format!("Invalid parameters: {}", e)))?;

    let nodes = node_service
        .get_related_nodes(
            &params.node_id,
            &params.relationship_name,
            &params.direction,
        )
        .await
        .map_err(|e| MCPError::internal_error(format!("Failed to get related nodes: {}", e)))?;

    let count = nodes.len();
    let related_nodes: Vec<Value> = nodes
        .into_iter()
        .map(|n| serde_json::to_value(n).unwrap_or(json!(null)))
        .collect();

    Ok(serde_json::to_value(GetRelatedNodesOutput {
        node_id: params.node_id,
        relationship_name: params.relationship_name,
        direction: params.direction,
        related_nodes,
        count,
    })
    .unwrap())
}

/// Get the complete relationship graph
///
/// # MCP Tool: get_relationship_graph
///
/// Returns a summary of all relationships defined in schemas. Useful for
/// NLP to understand the overall data model structure.
///
/// ## Returns
/// ```json
/// {
///   "edges": [
///     {"sourceType": "invoice", "relationshipName": "billed_to", "targetType": "customer"},
///     {"sourceType": "task", "relationshipName": "assigned_to", "targetType": "person"}
///   ],
///   "totalEdges": 2
/// }
/// ```
pub async fn handle_get_relationship_graph(
    node_service: &Arc<NodeService>,
    _params: Value,
) -> Result<Value, MCPError> {
    let graph = node_service.get_relationship_graph().await.map_err(|e| {
        MCPError::internal_error(format!("Failed to get relationship graph: {}", e))
    })?;

    let edges: Vec<RelationshipEdge> = graph
        .into_iter()
        .map(|(source, name, target)| RelationshipEdge {
            source_type: source,
            relationship_name: name,
            target_type: target,
        })
        .collect();

    let total = edges.len();

    Ok(serde_json::to_value(RelationshipGraphOutput {
        edges,
        total_edges: total,
    })
    .unwrap())
}

/// Get relationships pointing TO a node type
///
/// # MCP Tool: get_inbound_relationships
///
/// Discovers all relationships from other schemas that point TO the specified
/// node type. This is useful for NLP to understand reverse relationships without
/// mutating target schemas.
///
/// ## Parameters
/// - `target_type` - The node type to find inbound relationships for
///
/// ## Example
/// ```json
/// {
///   "target_type": "customer"
/// }
/// ```
///
/// ## Returns
/// ```json
/// {
///   "targetType": "customer",
///   "inboundRelationships": [
///     {
///       "sourceType": "invoice",
///       "relationshipName": "billed_to",
///       "reverseName": "invoices",
///       "cardinality": "one",
///       "edgeTable": "invoice_billed_to_customer"
///     }
///   ],
///   "count": 1
/// }
/// ```
pub async fn handle_get_inbound_relationships(
    node_service: &Arc<NodeService>,
    params: Value,
) -> Result<Value, MCPError> {
    let params: GetInboundRelationshipsParams = serde_json::from_value(params)
        .map_err(|e| MCPError::invalid_params(format!("Invalid parameters: {}", e)))?;

    let inbound = node_service
        .get_inbound_relationships(&params.target_type)
        .await
        .map_err(|e| {
            MCPError::internal_error(format!("Failed to get inbound relationships: {}", e))
        })?;

    let relationships: Vec<InboundRelationshipInfo> = inbound
        .into_iter()
        .map(|(source_type, rel)| {
            // Compute edge_table before moving other fields
            let edge_table = rel.compute_edge_table_name(&source_type);
            InboundRelationshipInfo {
                source_type,
                relationship_name: rel.name,
                reverse_name: rel.reverse_name,
                cardinality: format!("{:?}", rel.cardinality).to_lowercase(),
                edge_table,
            }
        })
        .collect();

    let count = relationships.len();

    Ok(serde_json::to_value(InboundRelationshipsOutput {
        target_type: params.target_type,
        inbound_relationships: relationships,
        count,
    })
    .unwrap())
}

/// Get all schemas with their relationships
///
/// # MCP Tool: get_all_schemas
///
/// Returns all schema definitions including their fields and relationships.
/// This is the primary entry point for NLP to understand the complete data model.
///
/// ## Returns
/// All schema nodes ordered by ID, with full field and relationship definitions.
pub async fn handle_get_all_schemas(
    node_service: &Arc<NodeService>,
    _params: Value,
) -> Result<Value, MCPError> {
    let schemas = node_service
        .get_all_schemas()
        .await
        .map_err(|e| MCPError::internal_error(format!("Failed to get all schemas: {}", e)))?;

    Ok(serde_json::to_value(schemas).unwrap())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_relationship_params_deserialization() {
        let json = json!({
            "source_id": "invoice-001",
            "relationship_name": "billed_to",
            "target_id": "customer-acme",
            "edge_data": {"billing_date": "2025-01-15"}
        });

        let params: CreateRelationshipParams = serde_json::from_value(json).unwrap();
        assert_eq!(params.source_id, "invoice-001");
        assert_eq!(params.relationship_name, "billed_to");
        assert_eq!(params.target_id, "customer-acme");
        assert!(params.edge_data.is_some());
    }

    #[test]
    fn test_get_related_nodes_params_default_direction() {
        let json = json!({
            "node_id": "invoice-001",
            "relationship_name": "billed_to"
        });

        let params: GetRelatedNodesParams = serde_json::from_value(json).unwrap();
        assert_eq!(params.direction, "out");
    }

    #[test]
    fn test_relationship_edge_serialization() {
        let edge = RelationshipEdge {
            source_type: "invoice".to_string(),
            relationship_name: "billed_to".to_string(),
            target_type: "customer".to_string(),
        };

        let json = serde_json::to_value(&edge).unwrap();
        assert_eq!(json["sourceType"], "invoice");
        assert_eq!(json["relationshipName"], "billed_to");
        assert_eq!(json["targetType"], "customer");
    }
}
