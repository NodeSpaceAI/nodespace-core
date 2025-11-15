//! Schema Management Service
//!
//! This service provides operations for managing user-defined entity schemas
//! using the Pure JSON schema-as-node pattern. Schemas are stored as regular
//! nodes with `node_type = 'schema'` and follow the convention `id = type_name`.
//!
//! ## Core Functionality
//!
//! - Get schema definitions by schema ID
//! - Add/remove user fields (protection enforced)
//! - Extend/remove enum values (user_values only)
//! - Automatic version incrementing on schema changes
//!
//! ## Protection Enforcement
//!
//! - **Core fields**: Cannot be modified or deleted (UI depends on these)
//! - **User fields**: Fully modifiable/deletable by users
//! - **System fields**: Auto-managed, read-only (future use)
//!
//! ## Example Usage
//!
//! ```no_run
//! # use nodespace_core::services::{NodeService, SchemaService};
//! # use nodespace_core::models::schema::{SchemaField, ProtectionLevel};
//! # use std::sync::Arc;
//! # #[tokio::main]
//! # async fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let node_service = Arc::new(NodeService::new(db)?);
//! let schema_service = SchemaService::new(node_service);
//!
//! // Get schema definition
//! let schema = schema_service.get_schema("task").await?;
//! println!("Task schema version: {}", schema.version);
//!
//! // Add a new user field
//! let priority_field = SchemaField {
//!     name: "priority".to_string(),
//!     field_type: "number".to_string(),
//!     protection: ProtectionLevel::User,
//!     indexed: false,
//!     required: Some(false),
//!     // ... other fields
//! };
//! schema_service.add_field("task", priority_field).await?;
//!
//! // Extend an enum field
//! schema_service.extend_enum_field("task", "status", "BLOCKED".to_string()).await?;
//! # Ok(())
//! # }
//! ```

use crate::models::schema::{ProtectionLevel, SchemaDefinition, SchemaField};
use crate::models::NodeUpdate;
use crate::services::node_service::NodeService;
use crate::services::NodeServiceError;
use std::sync::Arc;

/// Service for managing user-defined entity schemas
///
/// Wraps NodeService to provide specialized operations for schema nodes,
/// enforcing protection levels and maintaining version integrity.
pub struct SchemaService<C = surrealdb::engine::local::Db>
where
    C: surrealdb::Connection,
{
    node_service: Arc<NodeService<C>>,
}

impl<C> SchemaService<C>
where
    C: surrealdb::Connection,
{
    /// Create a new SchemaService
    ///
    /// # Arguments
    ///
    /// * `node_service` - NodeService instance for node operations
    pub fn new(node_service: Arc<NodeService<C>>) -> Self {
        Self { node_service }
    }

    /// Get schema definition by schema ID
    ///
    /// Schemas follow the convention: `id = type_name`, `node_type = "schema"`.
    ///
    /// # Arguments
    ///
    /// * `schema_id` - The schema ID (e.g., "task", "person")
    ///
    /// # Returns
    ///
    /// The parsed SchemaDefinition from the node's properties
    ///
    /// # Errors
    ///
    /// - `NodeNotFound`: Schema node doesn't exist
    /// - `InvalidNodeType`: Node exists but is not a schema node
    /// - `SerializationError`: Properties cannot be parsed as SchemaDefinition
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use nodespace_core::services::SchemaService;
    /// # async fn example(service: SchemaService) -> Result<(), Box<dyn std::error::Error>> {
    /// let schema = service.get_schema("task").await?;
    /// println!("Fields: {}", schema.fields.len());
    /// # Ok(())
    /// # }
    /// ```
    pub async fn get_schema(&self, schema_id: &str) -> Result<SchemaDefinition, NodeServiceError> {
        let node = self
            .node_service
            .get_node(schema_id)
            .await?
            .ok_or_else(|| NodeServiceError::node_not_found(schema_id))?;

        if node.node_type != "schema" {
            return Err(NodeServiceError::invalid_update(format!(
                "Node '{}' is not a schema (type: {})",
                schema_id, node.node_type
            )));
        }

        let schema: SchemaDefinition = serde_json::from_value(node.properties)
            .map_err(|e| NodeServiceError::serialization_error(e.to_string()))?;

        Ok(schema)
    }

    /// Add a new field to a schema
    ///
    /// Only user-protected fields can be added. Attempting to add core or system
    /// fields will return an error.
    ///
    /// The schema version is automatically incremented.
    ///
    /// # Arguments
    ///
    /// * `schema_id` - The schema ID to modify
    /// * `field` - The field definition to add
    ///
    /// # Errors
    ///
    /// - `InvalidUpdate`: Field has non-user protection level
    /// - `InvalidUpdate`: Field name already exists in schema
    /// - Any errors from `get_schema` or `update_schema`
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use nodespace_core::services::SchemaService;
    /// # use nodespace_core::models::schema::{SchemaField, ProtectionLevel};
    /// # async fn example(service: SchemaService) -> Result<(), Box<dyn std::error::Error>> {
    /// let field = SchemaField {
    ///     name: "custom:priority".to_string(),  // User fields must have namespace prefix
    ///     field_type: "number".to_string(),
    ///     protection: ProtectionLevel::User,
    ///     indexed: false,
    ///     required: Some(false),
    ///     extensible: None,
    ///     default: Some(serde_json::json!(0)),
    ///     core_values: None,
    ///     user_values: None,
    ///     description: Some("Task priority".to_string()),
    ///     item_type: None,
    /// };
    /// service.add_field("task", field).await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn add_field(
        &self,
        schema_id: &str,
        field: SchemaField,
    ) -> Result<(), NodeServiceError> {
        // Namespace validation: Enforce prefix requirements
        if field.protection == ProtectionLevel::Core {
            // Core fields must NOT have namespace prefix
            if field.name.contains(':') {
                return Err(NodeServiceError::invalid_update(
                    "Core properties cannot use namespace prefix. \
                     Core fields use simple names like 'status', 'priority', 'due_date'."
                        .to_string(),
                ));
            }
        } else {
            // User/plugin fields MUST have namespace prefix
            let valid_prefixes = ["custom:", "org:", "plugin:"];
            let has_valid_prefix = valid_prefixes
                .iter()
                .any(|prefix| field.name.starts_with(prefix));

            if !has_valid_prefix {
                return Err(NodeServiceError::invalid_update(format!(
                    "User properties must use namespace prefix to prevent conflicts with future core properties.\n\
                     Valid namespaces:\n\
                     - 'custom:{}' for personal custom properties\n\
                     - 'org:{}' for organization-specific properties\n\
                     - 'plugin:name:{}' for plugin-provided properties\n\n\
                     See: docs/architecture/development/schema-management-implementation-guide.md\n\
                     This ensures your custom properties will never conflict with future NodeSpace core features.",
                    field.name, field.name, field.name
                )));
            }
        }

        // Protection: Only user fields can be added
        if field.protection != ProtectionLevel::User {
            return Err(NodeServiceError::invalid_update(format!(
                "Can only add user-protected fields. Field '{}' has protection: {:?}",
                field.name, field.protection
            )));
        }

        let mut schema = self.get_schema(schema_id).await?;

        // Check if field already exists
        if schema.fields.iter().any(|f| f.name == field.name) {
            return Err(NodeServiceError::invalid_update(format!(
                "Field '{}' already exists in schema '{}'",
                field.name, schema_id
            )));
        }

        // Add field and bump version
        schema.fields.push(field);
        schema.version += 1;

        // Update schema node
        self.update_schema(schema_id, schema).await?;

        Ok(())
    }

    /// Remove a field from a schema
    ///
    /// Only user-protected fields can be removed. Attempting to remove core or
    /// system fields will return an error.
    ///
    /// The schema version is automatically incremented.
    ///
    /// # Arguments
    ///
    /// * `schema_id` - The schema ID to modify
    /// * `field_name` - The name of the field to remove
    ///
    /// # Errors
    ///
    /// - `InvalidUpdate`: Field not found in schema
    /// - `InvalidUpdate`: Field has non-user protection level
    /// - Any errors from `get_schema` or `update_schema`
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use nodespace_core::services::SchemaService;
    /// # async fn example(service: SchemaService) -> Result<(), Box<dyn std::error::Error>> {
    /// service.remove_field("task", "priority").await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn remove_field(
        &self,
        schema_id: &str,
        field_name: &str,
    ) -> Result<(), NodeServiceError> {
        let mut schema = self.get_schema(schema_id).await?;

        // Find the field
        let field_idx = schema
            .fields
            .iter()
            .position(|f| f.name == field_name)
            .ok_or_else(|| {
                NodeServiceError::invalid_update(format!(
                    "Field '{}' not found in schema '{}'",
                    field_name, schema_id
                ))
            })?;

        let field = &schema.fields[field_idx];

        // Protection: Only user fields can be removed
        if field.protection != ProtectionLevel::User {
            return Err(NodeServiceError::invalid_update(format!(
                "Cannot remove field '{}' with protection level {:?}. Only user fields can be removed.",
                field_name, field.protection
            )));
        }

        // Remove field and bump version
        schema.fields.remove(field_idx);
        schema.version += 1;

        // Update schema node
        self.update_schema(schema_id, schema).await?;

        Ok(())
    }

    /// Extend an enum field with a new value
    ///
    /// Adds a value to the `user_values` array of an enum field. The field must
    /// be marked as `extensible = true`.
    ///
    /// Core values cannot be added through this method (they are defined at
    /// schema creation time).
    ///
    /// The schema version is automatically incremented.
    ///
    /// # Arguments
    ///
    /// * `schema_id` - The schema ID to modify
    /// * `field_name` - The name of the enum field
    /// * `new_value` - The value to add to user_values
    ///
    /// # Errors
    ///
    /// - `InvalidUpdate`: Field not found
    /// - `InvalidUpdate`: Field is not an enum
    /// - `InvalidUpdate`: Field is not extensible
    /// - `InvalidUpdate`: Value already exists (in core or user values)
    /// - Any errors from `get_schema` or `update_schema`
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use nodespace_core::services::SchemaService;
    /// # async fn example(service: SchemaService) -> Result<(), Box<dyn std::error::Error>> {
    /// // Add "BLOCKED" to task.status enum
    /// service.extend_enum_field("task", "status", "BLOCKED".to_string()).await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn extend_enum_field(
        &self,
        schema_id: &str,
        field_name: &str,
        new_value: String,
    ) -> Result<(), NodeServiceError> {
        let mut schema = self.get_schema(schema_id).await?;

        // Check if value already exists (in core or user values) - do this BEFORE getting mutable ref
        let all_values = schema.get_enum_values(field_name).unwrap_or_default();
        if all_values.contains(&new_value) {
            return Err(NodeServiceError::invalid_update(format!(
                "Value '{}' already exists in enum '{}'",
                new_value, field_name
            )));
        }

        // Find the field
        let field = schema
            .fields
            .iter_mut()
            .find(|f| f.name == field_name)
            .ok_or_else(|| {
                NodeServiceError::invalid_update(format!(
                    "Field '{}' not found in schema '{}'",
                    field_name, schema_id
                ))
            })?;

        // Verify it's an enum field
        if field.field_type != "enum" {
            return Err(NodeServiceError::invalid_update(format!(
                "Field '{}' is not an enum (type: {})",
                field_name, field.field_type
            )));
        }

        // Verify it's extensible
        if !field.extensible.unwrap_or(false) {
            return Err(NodeServiceError::invalid_update(format!(
                "Enum field '{}' is not extensible",
                field_name
            )));
        }

        // Add to user_values
        field
            .user_values
            .get_or_insert_with(Vec::new)
            .push(new_value);
        schema.version += 1;

        // Update schema node
        self.update_schema(schema_id, schema).await?;

        Ok(())
    }

    /// Remove a value from an enum field
    ///
    /// Removes a value from the `user_values` array of an enum field. Core values
    /// cannot be removed - they are protected.
    ///
    /// The schema version is automatically incremented.
    ///
    /// # Arguments
    ///
    /// * `schema_id` - The schema ID to modify
    /// * `field_name` - The name of the enum field
    /// * `value` - The value to remove from user_values
    ///
    /// # Errors
    ///
    /// - `InvalidUpdate`: Field not found
    /// - `InvalidUpdate`: Value is a core value (cannot remove)
    /// - `InvalidUpdate`: Value not found in user_values
    /// - Any errors from `get_schema` or `update_schema`
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use nodespace_core::services::SchemaService;
    /// # async fn example(service: SchemaService) -> Result<(), Box<dyn std::error::Error>> {
    /// // Remove "BLOCKED" from task.status enum (user value only)
    /// service.remove_enum_value("task", "status", "BLOCKED").await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn remove_enum_value(
        &self,
        schema_id: &str,
        field_name: &str,
        value: &str,
    ) -> Result<(), NodeServiceError> {
        let mut schema = self.get_schema(schema_id).await?;

        // Find the field
        let field = schema
            .fields
            .iter_mut()
            .find(|f| f.name == field_name)
            .ok_or_else(|| {
                NodeServiceError::invalid_update(format!(
                    "Field '{}' not found in schema '{}'",
                    field_name, schema_id
                ))
            })?;

        // Check if value is in core_values (cannot remove)
        if let Some(core_vals) = &field.core_values {
            if core_vals.contains(&value.to_string()) {
                return Err(NodeServiceError::invalid_update(format!(
                    "Cannot remove core value '{}' from enum '{}'. Only user values can be removed.",
                    value, field_name
                )));
            }
        }

        // Remove from user_values
        if let Some(user_vals) = &mut field.user_values {
            if let Some(idx) = user_vals.iter().position(|v| v == value) {
                user_vals.remove(idx);
                schema.version += 1;

                // Update schema node
                self.update_schema(schema_id, schema).await?;
                return Ok(());
            }
        }

        Err(NodeServiceError::invalid_update(format!(
            "Value '{}' not found in user values of enum '{}'",
            value, field_name
        )))
    }

    /// Update schema node with new definition
    ///
    /// Internal helper method to persist schema changes.
    ///
    /// # Arguments
    ///
    /// * `schema_id` - The schema ID to update
    /// * `schema` - The new schema definition
    ///
    /// # Errors
    ///
    /// - `SerializationError`: Schema cannot be serialized to JSON
    /// - Any errors from `NodeService::update_node`
    async fn update_schema(
        &self,
        schema_id: &str,
        schema: SchemaDefinition,
    ) -> Result<(), NodeServiceError> {
        let properties = serde_json::to_value(&schema)
            .map_err(|e| NodeServiceError::serialization_error(e.to_string()))?;

        self.node_service
            .update_node(
                schema_id,
                NodeUpdate {
                    properties: Some(properties),
                    ..Default::default()
                },
            )
            .await?;

        Ok(())
    }

    /// Validate a node's properties against its schema definition
    ///
    /// Performs schema-driven validation of property values, including:
    /// - Enum value validation (core + user values)
    /// - Required field checking
    /// - Type validation (future enhancement)
    ///
    /// This method implements Step 2 of the hybrid validation approach:
    /// behaviors handle basic type checking, schemas handle value validation.
    ///
    /// # Arguments
    ///
    /// * `node` - The node to validate
    ///
    /// # Returns
    ///
    /// `Ok(())` if validation passes, or an error describing the validation failure
    ///
    /// # Errors
    ///
    /// - `InvalidUpdate`: Property value violates schema constraints
    /// - Any errors from `get_schema`
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use nodespace_core::services::{SchemaService, NodeService};
    /// # use nodespace_core::models::Node;
    /// # use serde_json::json;
    /// # async fn example(service: SchemaService) -> Result<(), Box<dyn std::error::Error>> {
    /// let node = Node::new(
    ///     "task".to_string(),
    ///     "Do something".to_string(),
    ///     None,
    ///     json!({"task": {"status": "INVALID_STATUS"}}),
    /// );
    ///
    /// // This will fail because "INVALID_STATUS" is not in the schema
    /// let result = service.validate_node_against_schema(&node).await;
    /// assert!(result.is_err());
    /// # Ok(())
    /// # }
    /// ```
    pub async fn validate_node_against_schema(
        &self,
        node: &crate::models::Node,
    ) -> Result<(), NodeServiceError> {
        // Try to get schema for this node type
        // If no schema exists, validation passes (not all types have schemas)
        let schema = match self.get_schema(&node.node_type).await {
            Ok(s) => s,
            Err(NodeServiceError::NodeNotFound { .. }) => return Ok(()), // No schema = no validation needed
            Err(e) => return Err(e),
        };

        // Get properties for this node type (supports both flat and nested formats)
        let node_props = node
            .properties
            .get(&node.node_type)
            .or(Some(&node.properties))
            .and_then(|p| p.as_object());

        // Validate each field in the schema
        for field in &schema.fields {
            let field_value = node_props.and_then(|props| props.get(&field.name));

            // Check required fields
            if field.required.unwrap_or(false) && field_value.is_none() {
                return Err(NodeServiceError::invalid_update(format!(
                    "Required field '{}' is missing from {} node",
                    field.name, node.node_type
                )));
            }

            // Validate enum fields
            if field.field_type == "enum" {
                if let Some(value) = field_value {
                    if let Some(value_str) = value.as_str() {
                        // Get all valid enum values (core + user)
                        let valid_values = schema.get_enum_values(&field.name).unwrap_or_default();

                        if !valid_values.contains(&value_str.to_string()) {
                            return Err(NodeServiceError::invalid_update(format!(
                                "Invalid value '{}' for enum field '{}'. Valid values: {}",
                                value_str,
                                field.name,
                                valid_values.join(", ")
                            )));
                        }
                    } else if !value.is_null() {
                        return Err(NodeServiceError::invalid_update(format!(
                            "Enum field '{}' must be a string or null",
                            field.name
                        )));
                    }
                }
            }

            // Future: Add more type validation (number ranges, string formats, etc.)
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::SurrealStore;
    use crate::models::Node;
    use serde_json::json;
    use tempfile::TempDir;

    async fn setup_test_service() -> (SchemaService, TempDir) {
        let temp_dir = TempDir::new().unwrap();
        let db_path = temp_dir.path().join("test.db");

        let store = Arc::new(SurrealStore::new(db_path).await.unwrap());
        let node_service = Arc::new(NodeService::new(store).unwrap());
        let schema_service = SchemaService::new(node_service);

        (schema_service, temp_dir)
    }

    async fn create_test_schema(service: &SchemaService) -> String {
        let schema = SchemaDefinition {
            is_core: false,
            version: 1,
            description: "Test widget schema".to_string(),
            fields: vec![
                SchemaField {
                    name: "status".to_string(),
                    field_type: "enum".to_string(),
                    protection: ProtectionLevel::Core,
                    core_values: Some(vec!["OPEN".to_string(), "DONE".to_string()]),
                    user_values: Some(vec![]),
                    indexed: true,
                    required: Some(true),
                    extensible: Some(true),
                    default: Some(json!("OPEN")),
                    description: Some("Widget status".to_string()),
                    item_type: None,
                },
                SchemaField {
                    name: "priority".to_string(),
                    field_type: "number".to_string(),
                    protection: ProtectionLevel::User,
                    core_values: None,
                    user_values: None,
                    indexed: false,
                    required: Some(false),
                    extensible: None,
                    default: Some(json!(0)),
                    description: Some("Widget priority".to_string()),
                    item_type: None,
                },
            ],
        };

        let schema_node = Node {
            id: "test_widget".to_string(),
            node_type: "schema".to_string(),
            content: "Test Widget".to_string(),
            parent_id: None,
            container_node_id: None,
            before_sibling_id: None,
            version: 1,
            created_at: chrono::Utc::now(),
            modified_at: chrono::Utc::now(),
            properties: serde_json::to_value(&schema).unwrap(),
            embedding_vector: None,
            mentions: Vec::new(),
            mentioned_by: Vec::new(),
        };

        service.node_service.create_node(schema_node).await.unwrap();

        "test_widget".to_string()
    }

    #[tokio::test]
    async fn test_get_schema() {
        let (service, _temp) = setup_test_service().await;
        let schema_id = create_test_schema(&service).await;

        let schema = service.get_schema(&schema_id).await.unwrap();

        assert_eq!(schema.version, 1);
        assert_eq!(schema.fields.len(), 2);
        assert_eq!(schema.fields[0].name, "status");
        assert_eq!(schema.fields[1].name, "priority");
    }

    #[tokio::test]
    async fn test_get_schema_not_found() {
        let (service, _temp) = setup_test_service().await;

        let result = service.get_schema("nonexistent").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_add_user_field_with_custom_namespace_succeeds() {
        let (service, _temp) = setup_test_service().await;
        let schema_id = create_test_schema(&service).await;

        let new_field = SchemaField {
            name: "custom:due_date".to_string(), // User field with namespace prefix
            field_type: "string".to_string(),
            protection: ProtectionLevel::User,
            core_values: None,
            user_values: None,
            indexed: false,
            required: Some(false),
            extensible: None,
            default: None,
            description: Some("Due date".to_string()),
            item_type: None,
        };

        service.add_field(&schema_id, new_field).await.unwrap();

        let schema = service.get_schema(&schema_id).await.unwrap();
        assert_eq!(schema.version, 2); // Version incremented
        assert_eq!(schema.fields.len(), 3);
        assert_eq!(schema.fields[2].name, "custom:due_date");
    }

    #[tokio::test]
    async fn test_add_user_field_without_namespace_rejected() {
        let (service, _temp) = setup_test_service().await;
        let schema_id = create_test_schema(&service).await;

        let invalid_field = SchemaField {
            name: "estimatedHours".to_string(), // Missing namespace prefix
            field_type: "number".to_string(),
            protection: ProtectionLevel::User,
            core_values: None,
            user_values: None,
            indexed: false,
            required: Some(false),
            extensible: None,
            default: None,
            description: Some("Estimated hours".to_string()),
            item_type: None,
        };

        let result = service.add_field(&schema_id, invalid_field).await;
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("User properties must use namespace prefix"));
        assert!(err_msg.contains("custom:estimatedHours"));
        assert!(err_msg.contains("org:estimatedHours"));
        assert!(err_msg.contains("plugin:name:estimatedHours"));
    }

    #[tokio::test]
    async fn test_add_user_field_with_custom_namespace() {
        let (service, _temp) = setup_test_service().await;
        let schema_id = create_test_schema(&service).await;

        let field = SchemaField {
            name: "custom:estimatedHours".to_string(),
            field_type: "number".to_string(),
            protection: ProtectionLevel::User,
            core_values: None,
            user_values: None,
            indexed: false,
            required: Some(false),
            extensible: None,
            default: Some(json!(8)),
            description: Some("Estimated hours".to_string()),
            item_type: None,
        };

        service.add_field(&schema_id, field).await.unwrap();

        let schema = service.get_schema(&schema_id).await.unwrap();
        assert_eq!(schema.fields.len(), 3);
        assert_eq!(schema.fields[2].name, "custom:estimatedHours");
    }

    #[tokio::test]
    async fn test_add_user_field_with_org_namespace() {
        let (service, _temp) = setup_test_service().await;
        let schema_id = create_test_schema(&service).await;

        let field = SchemaField {
            name: "org:departmentCode".to_string(),
            field_type: "string".to_string(),
            protection: ProtectionLevel::User,
            core_values: None,
            user_values: None,
            indexed: true,
            required: Some(false),
            extensible: None,
            default: None,
            description: Some("Department code".to_string()),
            item_type: None,
        };

        service.add_field(&schema_id, field).await.unwrap();

        let schema = service.get_schema(&schema_id).await.unwrap();
        assert_eq!(schema.fields.len(), 3);
        assert_eq!(schema.fields[2].name, "org:departmentCode");
    }

    #[tokio::test]
    async fn test_add_user_field_with_plugin_namespace() {
        let (service, _temp) = setup_test_service().await;
        let schema_id = create_test_schema(&service).await;

        let field = SchemaField {
            name: "plugin:jira:issueId".to_string(),
            field_type: "string".to_string(),
            protection: ProtectionLevel::User,
            core_values: None,
            user_values: None,
            indexed: true,
            required: Some(false),
            extensible: None,
            default: None,
            description: Some("JIRA issue ID".to_string()),
            item_type: None,
        };

        service.add_field(&schema_id, field).await.unwrap();

        let schema = service.get_schema(&schema_id).await.unwrap();
        assert_eq!(schema.fields.len(), 3);
        assert_eq!(schema.fields[2].name, "plugin:jira:issueId");
    }

    #[tokio::test]
    async fn test_core_field_with_namespace_rejected() {
        let (service, _temp) = setup_test_service().await;
        let schema_id = create_test_schema(&service).await;

        let invalid_core_field = SchemaField {
            name: "custom:status".to_string(), // Core field should not have prefix
            field_type: "string".to_string(),
            protection: ProtectionLevel::Core,
            core_values: None,
            user_values: None,
            indexed: false,
            required: Some(false),
            extensible: None,
            default: None,
            description: None,
            item_type: None,
        };

        let result = service.add_field(&schema_id, invalid_core_field).await;
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("Core properties cannot use namespace prefix"));
    }

    #[tokio::test]
    async fn test_add_core_field_rejected() {
        let (service, _temp) = setup_test_service().await;
        let schema_id = create_test_schema(&service).await;

        let core_field = SchemaField {
            name: "bad_field".to_string(),
            field_type: "string".to_string(),
            protection: ProtectionLevel::Core,
            core_values: None,
            user_values: None,
            indexed: false,
            required: Some(false),
            extensible: None,
            default: None,
            description: None,
            item_type: None,
        };

        let result = service.add_field(&schema_id, core_field).await;
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Can only add user-protected fields"));
    }

    #[tokio::test]
    async fn test_remove_user_field() {
        let (service, _temp) = setup_test_service().await;
        let schema_id = create_test_schema(&service).await;

        service.remove_field(&schema_id, "priority").await.unwrap();

        let schema = service.get_schema(&schema_id).await.unwrap();
        assert_eq!(schema.version, 2); // Version incremented
        assert_eq!(schema.fields.len(), 1);
        assert_eq!(schema.fields[0].name, "status");
    }

    #[tokio::test]
    async fn test_remove_core_field_rejected() {
        let (service, _temp) = setup_test_service().await;
        let schema_id = create_test_schema(&service).await;

        let result = service.remove_field(&schema_id, "status").await;
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Only user fields can be removed"));
    }

    #[tokio::test]
    async fn test_extend_enum_field() {
        let (service, _temp) = setup_test_service().await;
        let schema_id = create_test_schema(&service).await;

        service
            .extend_enum_field(&schema_id, "status", "BLOCKED".to_string())
            .await
            .unwrap();

        let schema = service.get_schema(&schema_id).await.unwrap();
        assert_eq!(schema.version, 2); // Version incremented

        let values = schema.get_enum_values("status").unwrap();
        assert_eq!(values.len(), 3);
        assert!(values.contains(&"BLOCKED".to_string()));
    }

    #[tokio::test]
    async fn test_extend_non_enum_field_rejected() {
        let (service, _temp) = setup_test_service().await;
        let schema_id = create_test_schema(&service).await;

        let result = service
            .extend_enum_field(&schema_id, "priority", "HIGH".to_string())
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not an enum"));
    }

    #[tokio::test]
    async fn test_remove_user_enum_value() {
        let (service, _temp) = setup_test_service().await;
        let schema_id = create_test_schema(&service).await;

        // First add a user value
        service
            .extend_enum_field(&schema_id, "status", "BLOCKED".to_string())
            .await
            .unwrap();

        // Then remove it
        service
            .remove_enum_value(&schema_id, "status", "BLOCKED")
            .await
            .unwrap();

        let schema = service.get_schema(&schema_id).await.unwrap();
        assert_eq!(schema.version, 3); // Two version increments

        let values = schema.get_enum_values("status").unwrap();
        assert_eq!(values.len(), 2); // Back to core values only
        assert!(!values.contains(&"BLOCKED".to_string()));
    }

    #[tokio::test]
    async fn test_remove_core_enum_value_rejected() {
        let (service, _temp) = setup_test_service().await;
        let schema_id = create_test_schema(&service).await;

        let result = service
            .remove_enum_value(&schema_id, "status", "OPEN")
            .await;
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Cannot remove core value"));
    }

    #[tokio::test]
    async fn test_validate_node_against_schema_valid_enum() {
        let (service, _temp) = setup_test_service().await;
        let _schema_id = create_test_schema(&service).await;

        let node = Node::new(
            "test_widget".to_string(),
            "My widget".to_string(),
            None,
            json!({"test_widget": {"status": "OPEN", "priority": 5}}),
        );

        let result = service.validate_node_against_schema(&node).await;
        assert!(result.is_ok(), "Valid enum value should pass validation");
    }

    #[tokio::test]
    async fn test_validate_node_against_schema_invalid_enum() {
        let (service, _temp) = setup_test_service().await;
        let _schema_id = create_test_schema(&service).await;

        let node = Node::new(
            "test_widget".to_string(),
            "My widget".to_string(),
            None,
            json!({"test_widget": {"status": "INVALID_STATUS"}}),
        );

        let result = service.validate_node_against_schema(&node).await;
        assert!(result.is_err(), "Invalid enum value should fail validation");
        assert!(
            result.unwrap_err().to_string().contains("Invalid value"),
            "Error should mention invalid value"
        );
    }

    #[tokio::test]
    async fn test_validate_node_against_schema_user_extended_enum() {
        let (service, _temp) = setup_test_service().await;
        let schema_id = create_test_schema(&service).await;

        // Extend enum with user value
        service
            .extend_enum_field(&schema_id, "status", "BLOCKED".to_string())
            .await
            .unwrap();

        // Should accept user-added enum value
        let node = Node::new(
            "test_widget".to_string(),
            "My widget".to_string(),
            None,
            json!({"test_widget": {"status": "BLOCKED"}}),
        );

        let result = service.validate_node_against_schema(&node).await;
        assert!(
            result.is_ok(),
            "User-extended enum value should pass validation"
        );
    }

    #[tokio::test]
    async fn test_validate_node_against_schema_no_schema() {
        let (service, _temp) = setup_test_service().await;

        // Node type with no schema should pass validation
        let node = Node::new(
            "unknown_type".to_string(),
            "Content".to_string(),
            None,
            json!({"some_field": "value"}),
        );

        let result = service.validate_node_against_schema(&node).await;
        assert!(result.is_ok(), "Node with no schema should pass validation");
    }

    #[tokio::test]
    async fn test_validate_node_against_schema_required_field_missing() {
        let (service, _temp) = setup_test_service().await;
        let _schema_id = create_test_schema(&service).await;

        // Node missing required field (status is required in test schema)
        let node = Node::new(
            "test_widget".to_string(),
            "My widget".to_string(),
            None,
            json!({"test_widget": {"priority": 5}}), // Missing required "status" field
        );

        let result = service.validate_node_against_schema(&node).await;
        assert!(result.is_err(), "Missing required field should fail");
        assert!(
            result.unwrap_err().to_string().contains("Required field"),
            "Error should mention required field"
        );
    }

    #[tokio::test]
    async fn test_validate_node_against_schema_enum_must_be_string() {
        let (service, _temp) = setup_test_service().await;
        let _schema_id = create_test_schema(&service).await;

        // Enum field with non-string value
        let node = Node::new(
            "test_widget".to_string(),
            "My widget".to_string(),
            None,
            json!({"test_widget": {"status": 123}}), // status must be string, not number
        );

        let result = service.validate_node_against_schema(&node).await;
        assert!(
            result.is_err(),
            "Enum field with non-string value should fail"
        );
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("must be a string or null"),
            "Error should mention type requirement"
        );
    }

    #[tokio::test]
    async fn test_validate_node_against_schema_backward_compat_flat_properties() {
        let (service, _temp) = setup_test_service().await;
        let _schema_id = create_test_schema(&service).await;

        // Backward compatibility: flat properties format (old style)
        let node = Node::new(
            "test_widget".to_string(),
            "My widget".to_string(),
            None,
            json!({"status": "DONE"}), // Flat format, not nested under "test_widget"
        );

        let result = service.validate_node_against_schema(&node).await;
        assert!(
            result.is_ok(),
            "Flat properties format should work for backward compatibility"
        );
    }
}
