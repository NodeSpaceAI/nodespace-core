//! Schema read commands for retrieving entity schemas
//!
//! Schema operations proxy through the in-process gRPC server
//! (nodespace-daemon) instead of calling `packages/core` directly.
//!
//! This module provides read-only schema commands:
//! - `get_all_schemas` - List all schemas
//! - `get_schema_definition` - Get a specific schema by ID
//!
//! Both return the `SchemaNode` wire type exactly as the daemon sent it: the
//! store fills its `relationships` and `extends`, and nothing here rebuilds a
//! schema from a node.

use crate::types::SchemaNode;
use nodespace_proto::nodespace::{GetAllSchemasRequest, GetSchemaDefinitionRequest};
use tauri::State;
use tonic::Request;

use super::nodes::{refusal_or, CommandError};
use crate::services::GrpcClient;

/// Decode one JSON-encoded `SchemaNode` from a schema read.
fn decode_schema(schema_json: &str) -> Result<SchemaNode, CommandError> {
    serde_json::from_str(schema_json).map_err(|e| CommandError {
        message: format!("Failed to decode schema: {}", e),
        code: "SCHEMA_SERVICE_ERROR".to_string(),
        details: None,
        conflict_data: None,
        requires_extension: None,
    })
}

/// Get all schemas
///
/// Retrieves every schema (both core and custom), each with the fields,
/// relationships and parent it declares itself.
///
/// # Returns
/// * `Ok(Vec<SchemaNode>)` - Every schema
/// * `Err(CommandError)` - Error if retrieval or decoding fails
#[tauri::command]
pub async fn get_all_schemas(
    client: State<'_, GrpcClient>,
) -> Result<Vec<SchemaNode>, CommandError> {
    let mut c = client.client().await;
    let resp = c
        .get_all_schemas(Request::new(GetAllSchemasRequest {}))
        .await
        .map_err(|s| {
            refusal_or(s, |s| CommandError {
                message: format!("Failed to retrieve schemas: {}", s.message()),
                code: "SCHEMA_SERVICE_ERROR".to_string(),
                details: Some(format!("{:?}", s.code())),
                conflict_data: None,
                requires_extension: None,
            })
        })?;

    resp.into_inner()
        .schemas_json
        .iter()
        .map(|schema_json| decode_schema(schema_json))
        .collect()
}

/// Get schema by ID
///
/// Retrieves the schema with its effective fields and relationships (its own
/// and those it inherits) and the parent it extends.
///
/// # Arguments
/// * `schema_id` - ID of the schema to retrieve (e.g., "task", "person")
///
/// # Returns
/// * `Ok(SchemaNode)` - The schema
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
            refusal_or(s, |s| {
                if s.code() == tonic::Code::NotFound {
                    CommandError {
                        message: format!("Schema '{}' not found", schema_id),
                        code: "SCHEMA_NOT_FOUND".to_string(),
                        details: None,
                        conflict_data: None,
                        requires_extension: None,
                    }
                } else {
                    CommandError {
                        message: format!("Schema operation failed: {}", s.message()),
                        code: "SCHEMA_SERVICE_ERROR".to_string(),
                        details: Some(format!("{:?}", s.code())),
                        conflict_data: None,
                        requires_extension: None,
                    }
                }
            })
        })?;

    decode_schema(&resp.into_inner().schema_json)
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
            requires_extension: None,
        };

        let json = serde_json::to_string(&err).unwrap();
        assert!(json.contains("Test error"));
        assert!(json.contains("TEST_ERROR"));
        assert!(json.contains("Debug info"));
    }

    /// The command hands the frontend the daemon's `SchemaNode` unchanged:
    /// relationships, parent and templates are all there, typed.
    #[test]
    fn test_decode_schema_keeps_relationships_and_extends() {
        let wire = serde_json::json!({
            "id": "bug",
            "nodeType": "schema",
            "content": "Bug",
            "version": 2,
            "createdAt": "2026-05-17T12:00:00Z",
            "modifiedAt": "2026-05-17T12:00:00Z",
            "properties": {},
            "lifecycleStatus": "active",
            "isCore": false,
            "extends": "ticket",
            "schemaVersion": 1,
            "fields": [{ "name": "severity", "type": "text", "friendlyName": "Severity" }],
            "relationships": [{
                "name": "owned_by",
                "targetType": "owner",
                "direction": "out",
                "cardinality": "one",
                "reverseName": "tickets",
                "reverseCardinality": "many"
            }],
            "titleTemplate": "{severity}"
        });

        let schema = decode_schema(&wire.to_string()).unwrap();
        assert_eq!(schema.extends.as_deref(), Some("ticket"));
        assert_eq!(schema.relationships[0].name, "owned_by");
        assert_eq!(schema.title_template.as_deref(), Some("{severity}"));

        // What the frontend receives is what the daemon sent.
        let sent = serde_json::to_value(&schema).unwrap();
        assert_eq!(sent["extends"], "ticket");
        assert_eq!(sent["relationships"], wire["relationships"]);
        assert_eq!(sent["properties"], serde_json::json!({}));
    }

    #[test]
    fn test_decode_schema_reports_a_malformed_schema() {
        let err = decode_schema("{\"id\": 1}").unwrap_err();
        assert_eq!(err.code, "SCHEMA_SERVICE_ERROR");
    }

    #[test]
    fn test_command_error_without_details() {
        let err = CommandError {
            message: "Simple error".to_string(),
            code: "SIMPLE".to_string(),
            details: None,
            conflict_data: None,
            requires_extension: None,
        };

        let json = serde_json::to_string(&err).unwrap();
        assert!(json.contains("Simple error"));
        assert!(!json.contains("details"));
    }
}
