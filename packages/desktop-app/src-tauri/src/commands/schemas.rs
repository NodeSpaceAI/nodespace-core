//! Schema read commands for retrieving entity schemas
//!
//! As of Issue #690, schema operations use NodeService with strongly-typed SchemaNode.
//! Schema mutation operations (add_field, remove_field, etc.) were removed
//! as they weren't used by any UI components.
//!
//! This module provides read-only schema commands:
//! - `get_all_schemas` - List all schema nodes (returns SchemaNode[] with typed fields)
//! - `get_schema_definition` - Get a specific schema by ID (returns SchemaNode with typed fields)

use nodespace_core::services::NodeServiceError;
use nodespace_core::{NodeQuery, NodeService, SchemaNode};
use serde::Serialize;
use tauri::State;

/// Structured error type for Tauri commands
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandError {
    pub message: String,
    pub code: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<String>,
}

impl From<NodeServiceError> for CommandError {
    fn from(err: NodeServiceError) -> Self {
        CommandError {
            message: format!("Schema operation failed: {}", err),
            code: "SCHEMA_SERVICE_ERROR".to_string(),
            details: Some(err.to_string()),
        }
    }
}

/// Get all schema nodes with typed fields
///
/// Retrieves all schema nodes (both core and custom) for plugin auto-registration.
/// Returns SchemaNode[] with typed top-level fields (isCore, schemaVersion, description, fields).
///
/// # Returns
/// * `Ok(Vec<SchemaNode>)` - Array of schema nodes with typed fields
/// * `Err(CommandError)` - Error if retrieval fails
#[tauri::command]
pub async fn get_all_schemas(
    service: State<'_, NodeService>,
) -> Result<Vec<SchemaNode>, CommandError> {
    // Query all schema nodes and wrap in SchemaNode for typed serialization
    let query = NodeQuery {
        node_type: Some("schema".to_string()),
        ..Default::default()
    };
    let nodes = service
        .query_nodes_simple(query)
        .await
        .map_err(CommandError::from)?;

    // Convert to SchemaNode for typed serialization
    let schema_nodes: Vec<SchemaNode> = nodes
        .into_iter()
        .filter_map(|node| SchemaNode::from_node(node).ok())
        .collect();

    Ok(schema_nodes)
}

/// Get schema by ID with typed fields
///
/// Retrieves the complete schema including all fields, protection levels,
/// and metadata. Returns SchemaNode with typed top-level fields.
///
/// # Arguments
/// * `schema_id` - ID of the schema to retrieve (e.g., "task", "person")
///
/// # Returns
/// * `Ok(SchemaNode)` - Schema with typed fields (isCore, schemaVersion, description, fields)
/// * `Err(CommandError)` - Error if schema not found
#[tauri::command]
pub async fn get_schema_definition(
    service: State<'_, NodeService>,
    schema_id: String,
) -> Result<SchemaNode, CommandError> {
    let schema_node = service
        .get_schema_node(&schema_id)
        .await
        .map_err(CommandError::from)?
        .ok_or_else(|| CommandError {
            message: format!("Schema '{}' not found", schema_id),
            code: "SCHEMA_NOT_FOUND".to_string(),
            details: None,
        })?;

    // Return SchemaNode directly - custom Serialize impl outputs typed fields
    Ok(schema_node)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_command_error_serialization() {
        let err = CommandError {
            message: "Test error".to_string(),
            code: "TEST_ERROR".to_string(),
            details: Some("Debug info".to_string()),
        };

        let json = serde_json::to_string(&err).unwrap();
        assert!(json.contains("Test error"));
        assert!(json.contains("TEST_ERROR"));
        assert!(json.contains("Debug info"));
    }

    #[test]
    fn test_command_error_without_details() {
        let err = CommandError {
            message: "Simple error".to_string(),
            code: "SIMPLE".to_string(),
            details: None,
        };

        let json = serde_json::to_string(&err).unwrap();
        assert!(json.contains("Simple error"));
        assert!(!json.contains("details"));
    }
}
