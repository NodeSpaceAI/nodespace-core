//! Schema read commands for retrieving entity schemas
//!
//! Schema operations proxy through the in-process gRPC server
//! (nodespace-daemon) instead of calling `packages/core` directly.
//!
//! This module provides read-only schema commands:
//! - `get_all_schemas` - List all schema nodes (returns SchemaNode[] with typed fields)
//! - `get_schema_definition` - Get a specific schema by ID (returns SchemaNode with typed fields)

use crate::types::SchemaNode;
use nodespace_proto::nodespace::{GetAllSchemasRequest, GetSchemaDefinitionRequest};
use tauri::State;
use tonic::Request;

use super::nodes::{proto_node_data_to_node, CommandError};
use crate::services::GrpcClient;

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
    client: State<'_, GrpcClient>,
) -> Result<Vec<SchemaNode>, CommandError> {
    let mut c = client.client().await;
    let resp = c
        .get_all_schemas(Request::new(GetAllSchemasRequest {}))
        .await
        .map_err(|s| CommandError {
            message: format!("Failed to retrieve schemas: {}", s.message()),
            code: "SCHEMA_SERVICE_ERROR".to_string(),
            details: Some(format!("{:?}", s.code())),
            conflict_data: None,
        })?;

    let schema_nodes: Vec<SchemaNode> = resp
        .into_inner()
        .nodes
        .into_iter()
        .filter_map(|nd| {
            proto_node_data_to_node(nd)
                .ok()
                .and_then(|node| SchemaNode::from_node(node).ok())
        })
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
    client: State<'_, GrpcClient>,
    schema_id: String,
) -> Result<SchemaNode, CommandError> {
    let mut c = client.client().await;
    let resp = c
        .get_schema_definition(Request::new(GetSchemaDefinitionRequest {
            schema_id: schema_id.clone(),
        }))
        .await
        .map_err(|s| {
            if s.code() == tonic::Code::NotFound {
                CommandError {
                    message: format!("Schema '{}' not found", schema_id),
                    code: "SCHEMA_NOT_FOUND".to_string(),
                    details: None,
                    conflict_data: None,
                }
            } else {
                CommandError {
                    message: format!("Schema operation failed: {}", s.message()),
                    code: "SCHEMA_SERVICE_ERROR".to_string(),
                    details: Some(format!("{:?}", s.code())),
                    conflict_data: None,
                }
            }
        })?;

    let nd = resp.into_inner().node_data.ok_or_else(|| CommandError {
        message: "gRPC GetSchemaDefinition response missing node_data".to_string(),
        code: "GRPC_ERROR".to_string(),
        details: None,
        conflict_data: None,
    })?;

    let node = proto_node_data_to_node(nd)?;

    SchemaNode::from_node(node).map_err(|e| CommandError {
        message: format!("Failed to parse schema node: {}", e),
        code: "SCHEMA_SERVICE_ERROR".to_string(),
        details: Some(format!("{:?}", e)),
        conflict_data: None,
    })
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
            conflict_data: None,
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
            conflict_data: None,
        };

        let json = serde_json::to_string(&err).unwrap();
        assert!(json.contains("Simple error"));
        assert!(!json.contains("details"));
    }
}
