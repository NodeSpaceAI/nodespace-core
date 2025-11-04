//! Schema operation commands for managing entity schemas
//!
//! This module provides Tauri commands for schema management operations,
//! serving as thin wrappers around SchemaService. These commands handle:
//! - Retrieving schema definitions
//! - Adding/removing user fields (with protection enforcement)
//! - Extending/removing enum values (user values only)
//!
//! ## Architecture Pattern
//!
//! These commands follow the established dual entry point pattern:
//! ```
//! Tauri Commands (desktop app)  ───┐
//!                                  ├──▶ SchemaService (shared backend)
//! MCP Handlers (external AI)    ───┘
//! ```
//!
//! Both Tauri commands and MCP handlers call the same SchemaService to ensure
//! consistent behavior and avoid logic duplication.

use nodespace_core::models::schema::{ProtectionLevel, SchemaDefinition, SchemaField};
use nodespace_core::services::{NodeServiceError, SchemaService};
use serde::{Deserialize, Serialize};
use tauri::State;

/// Structured error type for Tauri commands
///
/// Provides better observability and debugging by including error codes
/// and optional details alongside user-facing messages.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandError {
    /// User-facing error message
    pub message: String,
    /// Machine-readable error code
    pub code: String,
    /// Optional detailed error information for debugging
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<String>,
}

impl From<NodeServiceError> for CommandError {
    fn from(err: NodeServiceError) -> Self {
        CommandError {
            message: format!("Schema operation failed: {}", err),
            code: "SCHEMA_SERVICE_ERROR".to_string(),
            // Use Display trait instead of Debug to avoid leaking internal details
            details: Some(err.to_string()),
        }
    }
}

/// Result of schema field mutation operations
///
/// Returned by add_field, remove_field, extend_enum, and remove_enum_value
/// to communicate the updated schema state.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SchemaFieldResult {
    /// ID of the modified schema
    pub schema_id: String,
    /// New schema version after the operation
    pub new_version: i32,
}

/// Input for adding a new field to a schema
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AddSchemaFieldInput {
    /// Unique name for the field
    pub name: String,
    /// Field type (e.g., "string", "number", "enum", "array")
    pub field_type: String,
    /// Whether this field should be indexed for faster queries
    pub indexed: bool,
    /// Whether this field is required
    pub required: Option<bool>,
    /// Default value for the field
    pub default: Option<serde_json::Value>,
    /// Human-readable field description
    pub description: Option<String>,
    /// For array fields, the type of items in the array
    pub item_type: Option<String>,
    /// For enum fields, initial user values
    pub enum_values: Option<Vec<String>>,
    /// For enum fields, whether users can extend with more values
    pub extensible: Option<bool>,
}

/// Get schema definition by schema ID
///
/// Retrieves the complete schema definition including all fields,
/// protection levels, and metadata.
///
/// # Arguments
/// * `service` - SchemaService instance from Tauri state
/// * `schema_id` - ID of the schema to retrieve (e.g., "task", "person")
///
/// # Returns
/// * `Ok(SchemaDefinition)` - Complete schema definition
/// * `Err(CommandError)` - Error if schema not found or operation fails
///
/// # Errors
/// Returns error if:
/// - Schema with given ID doesn't exist
/// - Node exists but is not a schema node
/// - Schema properties cannot be deserialized
///
/// # Example Frontend Usage
/// ```typescript
/// const schema = await invoke('get_schema_definition', {
///   schemaId: 'task'
/// });
/// console.log(`Task schema has ${schema.fields.length} fields`);
/// ```
#[tauri::command]
pub async fn get_schema_definition(
    service: State<'_, SchemaService>,
    schema_id: String,
) -> Result<SchemaDefinition, CommandError> {
    service.get_schema(&schema_id).await.map_err(Into::into)
}

/// Add a new field to a schema
///
/// Creates a new user-protected field in the specified schema. Only user
/// fields can be added through this command - attempting to add core or
/// system fields will return an error.
///
/// The schema version is automatically incremented on success.
///
/// # Arguments
/// * `service` - SchemaService instance from Tauri state
/// * `schema_id` - ID of the schema to modify
/// * `field` - Field definition to add
///
/// # Returns
/// * `Ok(SchemaFieldResult)` - Success with new schema version
/// * `Err(CommandError)` - Error if validation fails
///
/// # Errors
/// Returns error if:
/// - Schema doesn't exist
/// - Field name already exists in schema
/// - Field has non-user protection level
/// - Database operation fails
///
/// # Example Frontend Usage
/// ```typescript
/// const result = await invoke('add_schema_field', {
///   schemaId: 'task',
///   field: {
///     name: 'priority',
///     fieldType: 'number',
///     indexed: false,
///     required: false,
///     default: 0,
///     description: 'Task priority level'
///   }
/// });
/// console.log(`Schema updated to version ${result.newVersion}`);
/// ```
#[tauri::command]
pub async fn add_schema_field(
    service: State<'_, SchemaService>,
    schema_id: String,
    field: AddSchemaFieldInput,
) -> Result<SchemaFieldResult, CommandError> {
    // Build SchemaField from input
    let schema_field = SchemaField {
        name: field.name,
        field_type: field.field_type,
        protection: ProtectionLevel::User, // Only user fields can be added
        core_values: None,                 // User fields don't have core values
        user_values: field.enum_values,
        indexed: field.indexed,
        required: field.required,
        extensible: field.extensible,
        default: field.default,
        description: field.description,
        item_type: field.item_type,
    };

    // Add field to schema
    service.add_field(&schema_id, schema_field).await?;

    // Get updated schema to return new version
    let updated_schema = service.get_schema(&schema_id).await?;

    Ok(SchemaFieldResult {
        schema_id,
        new_version: updated_schema.version as i32,
    })
}

/// Remove a field from a schema
///
/// Removes a user-protected field from the specified schema. Core and system
/// fields cannot be removed through this command.
///
/// The schema version is automatically incremented on success.
///
/// # Arguments
/// * `service` - SchemaService instance from Tauri state
/// * `schema_id` - ID of the schema to modify
/// * `field_name` - Name of the field to remove
///
/// # Returns
/// * `Ok(SchemaFieldResult)` - Success with new schema version
/// * `Err(CommandError)` - Error if validation fails
///
/// # Errors
/// Returns error if:
/// - Schema doesn't exist
/// - Field doesn't exist in schema
/// - Field has non-user protection level
/// - Database operation fails
///
/// # Example Frontend Usage
/// ```typescript
/// const result = await invoke('remove_schema_field', {
///   schemaId: 'task',
///   fieldName: 'priority'
/// });
/// console.log(`Field removed, schema version ${result.newVersion}`);
/// ```
#[tauri::command]
pub async fn remove_schema_field(
    service: State<'_, SchemaService>,
    schema_id: String,
    field_name: String,
) -> Result<SchemaFieldResult, CommandError> {
    service.remove_field(&schema_id, &field_name).await?;

    // Get updated schema to return new version
    let updated_schema = service.get_schema(&schema_id).await?;

    Ok(SchemaFieldResult {
        schema_id,
        new_version: updated_schema.version as i32,
    })
}

/// Extend an enum field with a new value
///
/// Adds a new value to the user_values array of an extensible enum field.
/// The field must be marked as extensible and the value must not already exist.
///
/// Core enum values cannot be added through this method - they are defined
/// at schema creation time and are protected.
///
/// The schema version is automatically incremented on success.
///
/// # Arguments
/// * `service` - SchemaService instance from Tauri state
/// * `schema_id` - ID of the schema to modify
/// * `field_name` - Name of the enum field to extend
/// * `value` - New value to add to user_values
///
/// # Returns
/// * `Ok(SchemaFieldResult)` - Success with new schema version
/// * `Err(CommandError)` - Error if validation fails
///
/// # Errors
/// Returns error if:
/// - Schema doesn't exist
/// - Field doesn't exist or is not an enum
/// - Field is not marked as extensible
/// - Value already exists (in core or user values)
/// - Database operation fails
///
/// # Example Frontend Usage
/// ```typescript
/// const result = await invoke('extend_schema_enum', {
///   schemaId: 'task',
///   fieldName: 'status',
///   value: 'BLOCKED'
/// });
/// console.log(`Enum extended, schema version ${result.newVersion}`);
/// ```
#[tauri::command]
pub async fn extend_schema_enum(
    service: State<'_, SchemaService>,
    schema_id: String,
    field_name: String,
    value: String,
) -> Result<SchemaFieldResult, CommandError> {
    service
        .extend_enum_field(&schema_id, &field_name, value)
        .await?;

    // Get updated schema to return new version
    let updated_schema = service.get_schema(&schema_id).await?;

    Ok(SchemaFieldResult {
        schema_id,
        new_version: updated_schema.version as i32,
    })
}

/// Remove a value from an enum field
///
/// Removes a value from the user_values array of an enum field. Core values
/// are protected and cannot be removed through this command.
///
/// The schema version is automatically incremented on success.
///
/// # Arguments
/// * `service` - SchemaService instance from Tauri state
/// * `schema_id` - ID of the schema to modify
/// * `field_name` - Name of the enum field to modify
/// * `value` - Value to remove from user_values
///
/// # Returns
/// * `Ok(SchemaFieldResult)` - Success with new schema version
/// * `Err(CommandError)` - Error if validation fails
///
/// # Errors
/// Returns error if:
/// - Schema doesn't exist
/// - Field doesn't exist
/// - Value is a core value (protected)
/// - Value not found in user_values
/// - Database operation fails
///
/// # Example Frontend Usage
/// ```typescript
/// const result = await invoke('remove_schema_enum_value', {
///   schemaId: 'task',
///   fieldName: 'status',
///   value: 'BLOCKED'
/// });
/// console.log(`Value removed, schema version ${result.newVersion}`);
/// ```
#[tauri::command]
pub async fn remove_schema_enum_value(
    service: State<'_, SchemaService>,
    schema_id: String,
    field_name: String,
    value: String,
) -> Result<SchemaFieldResult, CommandError> {
    service
        .remove_enum_value(&schema_id, &field_name, &value)
        .await?;

    // Get updated schema to return new version
    let updated_schema = service.get_schema(&schema_id).await?;

    Ok(SchemaFieldResult {
        schema_id,
        new_version: updated_schema.version as i32,
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
        // Details field should be omitted when None
        assert!(!json.contains("details"));
    }

    #[test]
    fn test_schema_field_result_serialization() {
        let result = SchemaFieldResult {
            schema_id: "task".to_string(),
            new_version: 5,
        };

        let json = serde_json::to_string(&result).unwrap();
        assert!(json.contains("task"));
        assert!(json.contains("5"));
        assert!(json.contains("schemaId")); // camelCase
        assert!(json.contains("newVersion")); // camelCase
    }
}
