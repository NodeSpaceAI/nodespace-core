//! MCP Schema Management Handlers
//!
//! Provides specialized MCP tools for schema operations with better UX
//! than using generic update_node. Enforces protection levels and provides
//! clear error messages.

use crate::mcp::types::MCPError;
use crate::models::schema::{FieldDefinition, ProtectionLevel};
use crate::services::SchemaService;
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;

/// Parameters for add_schema_field MCP method
#[derive(Debug, Deserialize)]
pub struct AddSchemaFieldParams {
    /// Schema ID to modify (matches node_type)
    pub schema_id: String,
    /// Field name
    pub field_name: String,
    /// Field type (e.g., "string", "number", "boolean", "enum", "array")
    pub field_type: String,
    /// Whether this field should be indexed
    #[serde(default)]
    pub indexed: bool,
    /// Whether this field is required
    #[serde(default)]
    pub required: Option<bool>,
    /// Default value for the field
    #[serde(default)]
    pub default: Option<Value>,
    /// Field description
    #[serde(default)]
    pub description: Option<String>,
    /// For array fields, the type of items in the array
    #[serde(default)]
    pub item_type: Option<String>,
    /// For enum fields, the allowed values (added to user_values)
    #[serde(default)]
    pub enum_values: Option<Vec<String>>,
    /// For enum fields, whether users can extend with more values
    #[serde(default)]
    pub extensible: Option<bool>,
}

/// Parameters for remove_schema_field MCP method
#[derive(Debug, Deserialize)]
pub struct RemoveSchemaFieldParams {
    /// Schema ID to modify
    pub schema_id: String,
    /// Field name to remove
    pub field_name: String,
}

/// Parameters for extend_schema_enum MCP method
#[derive(Debug, Deserialize)]
pub struct ExtendSchemaEnumParams {
    /// Schema ID to modify
    pub schema_id: String,
    /// Enum field name
    pub field_name: String,
    /// New value to add to user_values
    pub value: String,
}

/// Parameters for remove_schema_enum_value MCP method
#[derive(Debug, Deserialize)]
pub struct RemoveSchemaEnumValueParams {
    /// Schema ID to modify
    pub schema_id: String,
    /// Enum field name
    pub field_name: String,
    /// Value to remove from user_values
    pub value: String,
}

/// Parameters for get_schema_definition MCP method
#[derive(Debug, Deserialize)]
pub struct GetSchemaDefinitionParams {
    /// Schema ID to retrieve
    pub schema_id: String,
}

/// Parameters for create_user_schema MCP method
#[derive(Debug, Deserialize)]
pub struct CreateUserSchemaParams {
    /// Schema type name (e.g., "invoice", "project")
    pub type_name: String,
    /// Human-readable display name
    pub display_name: String,
    /// List of field definitions
    pub fields: Vec<FieldDefinition>,
}

/// Add a new field to a schema
///
/// # MCP Tool Description
/// Add a new user-protected field to an existing schema. Core and system fields
/// cannot be added through MCP (only user fields allowed). The schema version
/// will be incremented automatically.
///
/// **IMPORTANT: Namespace Requirement**
/// All user-defined field names MUST include a namespace prefix to prevent conflicts
/// with future core properties. Valid namespace prefixes are:
/// - `custom:` - For personal custom properties (e.g., `custom:estimatedHours`)
/// - `org:` - For organization-specific properties (e.g., `org:departmentCode`)
/// - `plugin:` - For plugin-provided properties (e.g., `plugin:jira:issueId`)
///
/// Core properties (added by NodeSpace) use simple names without prefixes
/// (e.g., `status`, `priority`, `due_date`).
///
/// # Parameters
/// - `schema_id`: ID of the schema to modify (matches node_type)
/// - `field_name`: Name of the new field (MUST include namespace prefix: `custom:`, `org:`, or `plugin:`)
/// - `field_type`: Type of the field (string, number, boolean, enum, array)
/// - `indexed`: Whether to index this field for search (default: false)
/// - `required`: Whether this field is required (optional)
/// - `default`: Default value for the field (optional)
/// - `description`: Field description (optional)
/// - `item_type`: For array fields, the type of items (optional)
/// - `enum_values`: For enum fields, the allowed values (optional, added to user_values)
/// - `extensible`: For enum fields, whether users can add more values (optional)
///
/// # Returns
/// - `schema_id`: ID of the modified schema
/// - `new_version`: New schema version after adding the field
/// - `success`: true
///
/// # Errors
/// - `VALIDATION_ERROR`: If field name lacks namespace prefix, trying to add a non-user field, or field already exists
/// - `NODE_NOT_FOUND`: If schema doesn't exist
pub async fn handle_add_schema_field(
    schema_service: &Arc<SchemaService>,
    params: Value,
) -> Result<Value, MCPError> {
    let params: AddSchemaFieldParams = serde_json::from_value(params)
        .map_err(|e| MCPError::invalid_params(format!("Invalid parameters: {}", e)))?;

    // Build field definition
    let field = crate::models::schema::SchemaField {
        name: params.field_name.clone(),
        field_type: params.field_type,
        protection: ProtectionLevel::User, // Always user-protected when added via MCP
        core_values: None,
        user_values: params.enum_values,
        indexed: params.indexed,
        required: params.required,
        extensible: params.extensible,
        default: params.default,
        description: params.description,
        item_type: params.item_type,
        fields: None,
        item_fields: None,
    };

    // Add field via SchemaService
    schema_service
        .add_field(&params.schema_id, field)
        .await
        .map_err(|e| MCPError::validation_error(e.to_string()))?;

    // Get updated schema to return new version
    let schema = schema_service
        .get_schema(&params.schema_id)
        .await
        .map_err(|e| MCPError::internal_error(format!("Failed to retrieve schema: {}", e)))?;

    Ok(json!({
        "schema_id": params.schema_id,
        "new_version": schema.version,
        "success": true
    }))
}

/// Remove a field from a schema
///
/// # MCP Tool Description
/// Remove a user-protected field from an existing schema. Core and system fields
/// cannot be removed (protection enforcement). The schema version will be
/// incremented automatically.
///
/// # Parameters
/// - `schema_id`: ID of the schema to modify
/// - `field_name`: Name of the field to remove
///
/// # Returns
/// - `schema_id`: ID of the modified schema
/// - `new_version`: New schema version after removing the field
/// - `success`: true
///
/// # Errors
/// - `VALIDATION_ERROR`: If trying to remove a core/system field or field doesn't exist
/// - `NODE_NOT_FOUND`: If schema doesn't exist
pub async fn handle_remove_schema_field(
    schema_service: &Arc<SchemaService>,
    params: Value,
) -> Result<Value, MCPError> {
    let params: RemoveSchemaFieldParams = serde_json::from_value(params)
        .map_err(|e| MCPError::invalid_params(format!("Invalid parameters: {}", e)))?;

    // Remove field via SchemaService
    schema_service
        .remove_field(&params.schema_id, &params.field_name)
        .await
        .map_err(|e| MCPError::validation_error(e.to_string()))?;

    // Get updated schema to return new version
    let schema = schema_service
        .get_schema(&params.schema_id)
        .await
        .map_err(|e| MCPError::internal_error(format!("Failed to retrieve schema: {}", e)))?;

    Ok(json!({
        "schema_id": params.schema_id,
        "new_version": schema.version,
        "success": true
    }))
}

/// Extend an enum field with a new value
///
/// # MCP Tool Description
/// Add a new value to an enum field's user_values list. The enum field must be
/// marked as extensible. Core enum values cannot be modified. The schema version
/// will be incremented automatically.
///
/// # Parameters
/// - `schema_id`: ID of the schema to modify
/// - `field_name`: Name of the enum field
/// - `value`: New value to add to user_values
///
/// # Returns
/// - `schema_id`: ID of the modified schema
/// - `new_version`: New schema version after adding the value
/// - `success`: true
///
/// # Errors
/// - `VALIDATION_ERROR`: If field is not enum, not extensible, or value already exists
/// - `NODE_NOT_FOUND`: If schema or field doesn't exist
pub async fn handle_extend_schema_enum(
    schema_service: &Arc<SchemaService>,
    params: Value,
) -> Result<Value, MCPError> {
    let params: ExtendSchemaEnumParams = serde_json::from_value(params)
        .map_err(|e| MCPError::invalid_params(format!("Invalid parameters: {}", e)))?;

    // Extend enum via SchemaService
    schema_service
        .extend_enum_field(&params.schema_id, &params.field_name, params.value)
        .await
        .map_err(|e| MCPError::validation_error(e.to_string()))?;

    // Get updated schema to return new version
    let schema = schema_service
        .get_schema(&params.schema_id)
        .await
        .map_err(|e| MCPError::internal_error(format!("Failed to retrieve schema: {}", e)))?;

    Ok(json!({
        "schema_id": params.schema_id,
        "new_version": schema.version,
        "success": true
    }))
}

/// Remove a value from an enum field
///
/// # MCP Tool Description
/// Remove a value from an enum field's user_values list. Core enum values cannot
/// be removed (protection enforcement). The schema version will be incremented
/// automatically.
///
/// # Parameters
/// - `schema_id`: ID of the schema to modify
/// - `field_name`: Name of the enum field
/// - `value`: Value to remove from user_values
///
/// # Returns
/// - `schema_id`: ID of the modified schema
/// - `new_version`: New schema version after removing the value
/// - `success`: true
///
/// # Errors
/// - `VALIDATION_ERROR`: If trying to remove core value or value doesn't exist
/// - `NODE_NOT_FOUND`: If schema or field doesn't exist
pub async fn handle_remove_schema_enum_value(
    schema_service: &Arc<SchemaService>,
    params: Value,
) -> Result<Value, MCPError> {
    let params: RemoveSchemaEnumValueParams = serde_json::from_value(params)
        .map_err(|e| MCPError::invalid_params(format!("Invalid parameters: {}", e)))?;

    // Remove enum value via SchemaService
    schema_service
        .remove_enum_value(&params.schema_id, &params.field_name, &params.value)
        .await
        .map_err(|e| MCPError::validation_error(e.to_string()))?;

    // Get updated schema to return new version
    let schema = schema_service
        .get_schema(&params.schema_id)
        .await
        .map_err(|e| MCPError::internal_error(format!("Failed to retrieve schema: {}", e)))?;

    Ok(json!({
        "schema_id": params.schema_id,
        "new_version": schema.version,
        "success": true
    }))
}

/// Get schema definition
///
/// # MCP Tool Description
/// Retrieve the complete schema definition for a given schema ID. Returns the
/// parsed schema with all fields, protection levels, and metadata.
///
/// # Parameters
/// - `schema_id`: ID of the schema to retrieve
///
/// # Returns
/// - `schema`: Complete schema definition object with:
///   - `is_core`: Whether this is a core schema
///   - `version`: Current schema version
///   - `description`: Schema description
///   - `fields`: Array of field definitions with:
///     - `name`: Field name
///     - `type`: Field type
///     - `protection`: Protection level (core, user, system)
///     - `indexed`: Whether field is indexed
///     - Additional field-specific properties
///
/// # Errors
/// - `NODE_NOT_FOUND`: If schema doesn't exist
pub async fn handle_get_schema_definition(
    schema_service: &Arc<SchemaService>,
    params: Value,
) -> Result<Value, MCPError> {
    let params: GetSchemaDefinitionParams = serde_json::from_value(params)
        .map_err(|e| MCPError::invalid_params(format!("Invalid parameters: {}", e)))?;

    // Get schema via SchemaService
    let schema = schema_service
        .get_schema(&params.schema_id)
        .await
        .map_err(|_e| MCPError::node_not_found(&params.schema_id))?;

    // Serialize schema to JSON
    let schema_json = serde_json::to_value(&schema)
        .map_err(|e| MCPError::internal_error(format!("Failed to serialize schema: {}", e)))?;

    Ok(json!({
        "schema": schema_json
    }))
}

/// Create a new user-defined schema
///
/// # MCP Tool Description
/// Create a new user-defined schema with spoke table, relation tables, and schema
/// node atomically. The spoke table is created as SCHEMALESS for user types.
/// Reference fields automatically generate relation tables with auto-naming convention.
///
/// # Parameters
/// - `type_name`: Schema type name (e.g., "invoice", "project")
/// - `display_name`: Human-readable display name
/// - `fields`: Array of field definitions with:
///   - `name`: Field name
///   - `type`: Field type (string/number/boolean/date/json/object or reference type)
///   - `required`: Whether field is required (optional)
///   - `default`: Default value (optional)
///   - `schema`: For nested objects (optional)
///
/// # Field Types
/// - **Primitive types** (stored in spoke table): string, number, boolean, date, json, object
/// - **Reference types** (create relation tables): any other type (person, project, task, etc.)
///
/// # Relation Table Naming
/// Reference fields automatically generate relation tables with naming convention:
/// `{source_type}_{field_name}` (e.g., `invoice_customer`, `task_assignee`)
///
/// # Returns
/// - `schema_id`: ID of the created schema node
/// - `type_name`: The schema type name
/// - `success`: true
///
/// # Errors
/// - `VALIDATION_ERROR`: Schema already exists or referenced type doesn't exist
/// - `INTERNAL_ERROR`: Database operation failed
pub async fn handle_create_user_schema(
    schema_service: &Arc<SchemaService>,
    params: Value,
) -> Result<Value, MCPError> {
    let params: CreateUserSchemaParams = serde_json::from_value(params)
        .map_err(|e| MCPError::invalid_params(format!("Invalid parameters: {}", e)))?;

    // Create schema via SchemaService
    let schema_id = schema_service
        .create_user_schema(&params.type_name, &params.display_name, params.fields)
        .await
        .map_err(|e| MCPError::validation_error(e.to_string()))?;

    Ok(json!({
        "schema_id": schema_id,
        "type_name": params.type_name,
        "success": true
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::SurrealStore;
    use crate::NodeService;
    use tempfile::TempDir;

    async fn setup_test_service() -> (Arc<SchemaService>, Arc<NodeService>, TempDir) {
        let temp_dir = TempDir::new().unwrap();
        let db_path = temp_dir.path().join("test.db");

        let store = Arc::new(SurrealStore::new(db_path).await.unwrap());
        let node_service = Arc::new(NodeService::new(store).unwrap());
        let schema_service = Arc::new(SchemaService::new(node_service.clone()));
        (schema_service, node_service, temp_dir)
    }

    async fn create_test_schema(node_service: &NodeService) {
        // Create a minimal test schema
        use crate::models::schema::{SchemaDefinition, SchemaField};
        use crate::models::Node;

        let schema = SchemaDefinition {
            is_core: false,
            version: 1,
            description: "Test schema".to_string(),
            // Status values use lowercase format (Issue #670)
            fields: vec![SchemaField {
                name: "status".to_string(),
                field_type: "enum".to_string(),
                protection: ProtectionLevel::Core,
                core_values: Some(vec!["open".to_string(), "done".to_string()]),
                user_values: None,
                indexed: true,
                required: Some(true),
                extensible: Some(true),
                default: Some(json!("open")),
                description: Some("Task status".to_string()),
                item_type: None,
                fields: None,
                item_fields: None,
            }],
        };

        let schema_node = Node {
            id: "test_schema".to_string(),
            node_type: "schema".to_string(),
            content: "Test Schema".to_string(),
            version: 1,
            created_at: chrono::Utc::now(),
            modified_at: chrono::Utc::now(),
            properties: serde_json::to_value(&schema).unwrap(),
            embedding_vector: None,
            mentions: Vec::new(),
            mentioned_by: Vec::new(),
        };

        node_service.create_node(schema_node).await.unwrap();
    }

    #[tokio::test]
    async fn test_add_schema_field() {
        let (schema_service, node_service, _temp) = setup_test_service().await;
        create_test_schema(&node_service).await;

        let params = json!({
            "schema_id": "test_schema",
            "field_name": "custom:custom_field",  // User field with namespace prefix
            "field_type": "string",
            "indexed": true,
            "description": "Custom user field"
        });

        let result = handle_add_schema_field(&schema_service, params)
            .await
            .unwrap();

        assert_eq!(result["schema_id"], "test_schema");
        assert_eq!(result["new_version"], 2);
        assert_eq!(result["success"], true);

        // Verify field was added
        let schema = schema_service.get_schema("test_schema").await.unwrap();
        let field = schema
            .fields
            .iter()
            .find(|f| f.name == "custom:custom_field");
        assert!(field.is_some());
        assert_eq!(field.unwrap().protection, ProtectionLevel::User);
    }

    #[tokio::test]
    async fn test_add_schema_field_without_namespace_rejected() {
        let (schema_service, node_service, _temp) = setup_test_service().await;
        create_test_schema(&node_service).await;

        let params = json!({
            "schema_id": "test_schema",
            "field_name": "estimatedHours",  // Missing namespace prefix
            "field_type": "number",
            "indexed": false,
            "description": "Estimated hours"
        });

        let result = handle_add_schema_field(&schema_service, params).await;

        assert!(result.is_err());
        let error = result.unwrap_err();
        assert!(error
            .message
            .contains("User properties must use namespace prefix"));
    }

    #[tokio::test]
    async fn test_remove_schema_field() {
        let (schema_service, node_service, _temp) = setup_test_service().await;
        create_test_schema(&node_service).await;

        // First add a user field with namespace
        let add_params = json!({
            "schema_id": "test_schema",
            "field_name": "custom:temp_field",
            "field_type": "string",
            "indexed": false
        });
        handle_add_schema_field(&schema_service, add_params)
            .await
            .unwrap();

        // Then remove it
        let remove_params = json!({
            "schema_id": "test_schema",
            "field_name": "custom:temp_field"
        });

        let result = handle_remove_schema_field(&schema_service, remove_params)
            .await
            .unwrap();

        assert_eq!(result["schema_id"], "test_schema");
        assert_eq!(result["success"], true);

        // Verify field was removed
        let schema = schema_service.get_schema("test_schema").await.unwrap();
        let field = schema.fields.iter().find(|f| f.name == "custom:temp_field");
        assert!(field.is_none());
    }

    #[tokio::test]
    async fn test_remove_core_field_rejected() {
        let (schema_service, node_service, _temp) = setup_test_service().await;
        create_test_schema(&node_service).await;

        // Try to remove core field
        let params = json!({
            "schema_id": "test_schema",
            "field_name": "status"
        });

        let result = handle_remove_schema_field(&schema_service, params).await;

        assert!(result.is_err());
        let error = result.unwrap_err();
        assert!(error.message.contains("Cannot remove"));
    }

    #[tokio::test]
    async fn test_extend_schema_enum() {
        let (schema_service, node_service, _temp) = setup_test_service().await;
        create_test_schema(&node_service).await;

        // Extend status enum
        let params = json!({
            "schema_id": "test_schema",
            "field_name": "status",
            "value": "BLOCKED"
        });

        let result = handle_extend_schema_enum(&schema_service, params)
            .await
            .unwrap();

        assert_eq!(result["schema_id"], "test_schema");
        assert_eq!(result["success"], true);

        // Verify value was added to user_values
        let schema = schema_service.get_schema("test_schema").await.unwrap();
        let values = schema.get_enum_values("status").unwrap();
        assert!(values.contains(&"BLOCKED".to_string()));
    }

    #[tokio::test]
    async fn test_remove_schema_enum_value() {
        let (schema_service, node_service, _temp) = setup_test_service().await;
        create_test_schema(&node_service).await;

        // First add a value
        let add_params = json!({
            "schema_id": "test_schema",
            "field_name": "status",
            "value": "TEMP_STATUS"
        });
        handle_extend_schema_enum(&schema_service, add_params)
            .await
            .unwrap();

        // Then remove it
        let remove_params = json!({
            "schema_id": "test_schema",
            "field_name": "status",
            "value": "TEMP_STATUS"
        });

        let result = handle_remove_schema_enum_value(&schema_service, remove_params)
            .await
            .unwrap();

        assert_eq!(result["schema_id"], "test_schema");
        assert_eq!(result["success"], true);

        // Verify value was removed
        let schema = schema_service.get_schema("test_schema").await.unwrap();
        let values = schema.get_enum_values("status").unwrap();
        assert!(!values.contains(&"TEMP_STATUS".to_string()));
    }

    #[tokio::test]
    async fn test_remove_core_enum_value_rejected() {
        let (schema_service, node_service, _temp) = setup_test_service().await;
        create_test_schema(&node_service).await;

        // Try to remove core enum value (status uses lowercase format per Issue #670)
        let params = json!({
            "schema_id": "test_schema",
            "field_name": "status",
            "value": "open"
        });

        let result = handle_remove_schema_enum_value(&schema_service, params).await;

        assert!(result.is_err());
        let error = result.unwrap_err();
        assert!(error.message.contains("Cannot remove core value"));
    }

    #[tokio::test]
    async fn test_get_schema_definition() {
        let (schema_service, node_service, _temp) = setup_test_service().await;
        create_test_schema(&node_service).await;

        let params = json!({
            "schema_id": "test_schema"
        });

        let result = handle_get_schema_definition(&schema_service, params)
            .await
            .unwrap();

        assert!(result["schema"].is_object());
        let schema = &result["schema"];
        // Schema uses camelCase serialization (Issue #670)
        assert!(!schema["isCore"].as_bool().unwrap());
        assert_eq!(schema["version"].as_u64().unwrap(), 1);
        assert!(schema["fields"].is_array());
        assert!(!schema["fields"].as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn test_create_user_schema() {
        let (schema_service, _node_service, _temp) = setup_test_service().await;

        let params = json!({
            "type_name": "invoice",
            "display_name": "Invoice",
            "fields": [
                {
                    "name": "total",
                    "type": "number",
                    "required": true
                },
                {
                    "name": "description",
                    "type": "string",
                    "required": false
                }
            ]
        });

        let result = handle_create_user_schema(&schema_service, params)
            .await
            .unwrap();

        assert_eq!(result["type_name"], "invoice");
        assert_eq!(result["success"], true);
        assert!(result["schema_id"].is_string());

        // Verify schema was created
        let schema = schema_service.get_schema("invoice").await.unwrap();
        assert_eq!(schema.fields.len(), 2);
        assert_eq!(schema.fields[0].name, "total");
        assert_eq!(schema.fields[1].name, "description");
    }

    #[tokio::test]
    async fn test_create_user_schema_with_reference() {
        let (schema_service, _node_service, _temp) = setup_test_service().await;

        // First create invoice schema
        let invoice_params = json!({
            "type_name": "invoice",
            "display_name": "Invoice",
            "fields": [
                {
                    "name": "amount",
                    "type": "number",
                    "required": true
                }
            ]
        });

        handle_create_user_schema(&schema_service, invoice_params)
            .await
            .unwrap();

        // Then create expense with reference to invoice
        let expense_params = json!({
            "type_name": "expense",
            "display_name": "Expense",
            "fields": [
                {
                    "name": "description",
                    "type": "string",
                    "required": true
                },
                {
                    "name": "invoice_ref",
                    "type": "invoice",
                    "required": false
                }
            ]
        });

        let result = handle_create_user_schema(&schema_service, expense_params)
            .await
            .unwrap();

        assert_eq!(result["success"], true);

        // Verify relation field is stored as record
        let schema = schema_service.get_schema("expense").await.unwrap();
        assert_eq!(schema.fields[1].field_type, "record");
    }

    #[tokio::test]
    async fn test_create_user_schema_duplicate_rejected() {
        let (schema_service, _node_service, _temp) = setup_test_service().await;

        let params = json!({
            "type_name": "duplicate",
            "display_name": "Duplicate",
            "fields": [
                {
                    "name": "name",
                    "type": "string",
                    "required": true
                }
            ]
        });

        // First creation succeeds
        handle_create_user_schema(&schema_service, params.clone())
            .await
            .unwrap();

        // Second creation fails
        let result = handle_create_user_schema(&schema_service, params).await;
        assert!(result.is_err());
        let error = result.unwrap_err();
        assert!(error.message.contains("already exists"));
    }

    #[tokio::test]
    async fn test_create_user_schema_invalid_reference_rejected() {
        let (schema_service, _node_service, _temp) = setup_test_service().await;

        let params = json!({
            "type_name": "invoice",
            "display_name": "Invoice",
            "fields": [
                {
                    "name": "customer",
                    "type": "nonexistent_type",
                    "required": false
                }
            ]
        });

        let result = handle_create_user_schema(&schema_service, params).await;
        assert!(result.is_err());
        let error = result.unwrap_err();
        assert!(error.message.contains("does not exist"));
    }
}
