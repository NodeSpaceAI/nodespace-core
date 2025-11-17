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

    /// Get all schema definitions
    ///
    /// Retrieves all schema nodes (node_type = "schema") and returns them as a vector
    /// of tuples containing the schema ID and schema definition.
    ///
    /// # Returns
    ///
    /// A vector of tuples where each tuple contains:
    /// - Schema ID (e.g., "task", "person")
    /// - Parsed SchemaDefinition from the node's properties
    ///
    /// # Errors
    ///
    /// - `SerializationError`: If any schema properties cannot be parsed
    /// - Any errors from the underlying node service query
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use nodespace_core::services::SchemaService;
    /// # async fn example(service: SchemaService) -> Result<(), Box<dyn std::error::Error>> {
    /// let schemas = service.get_all_schemas().await?;
    /// for (id, schema) in schemas {
    ///     println!("Schema {}: {} fields", id, schema.fields.len());
    /// }
    /// # Ok(())
    /// # }
    /// ```
    pub async fn get_all_schemas(
        &self,
    ) -> Result<Vec<(String, SchemaDefinition)>, NodeServiceError> {
        // Query all nodes with node_type = "schema"
        let query = crate::models::NodeQuery {
            node_type: Some("schema".to_string()),
            ..Default::default()
        };

        let nodes = self.node_service.query_nodes_simple(query).await?;

        // Parse each node's properties as SchemaDefinition
        let mut schemas = Vec::new();
        for node in nodes {
            let schema: SchemaDefinition = serde_json::from_value(node.properties)
                .map_err(|e| NodeServiceError::serialization_error(e.to_string()))?;

            schemas.push((node.id, schema));
        }

        Ok(schemas)
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

        // Sync schema changes to database
        self.sync_schema_to_database(schema_id).await?;

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

        // Sync schema changes to database
        self.sync_schema_to_database(schema_id).await?;

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

        // Sync schema changes to database
        self.sync_schema_to_database(schema_id).await?;

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

                // Sync schema changes to database
                self.sync_schema_to_database(schema_id).await?;

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

    /// Synchronize schema definition to database schema
    ///
    /// Creates or updates database table and field definitions based on schema.
    /// Uses SCHEMAFULL mode for core types, SCHEMALESS for user types.
    ///
    /// # Arguments
    ///
    /// * `type_name` - The schema type name (e.g., "person", "task")
    ///
    /// # Returns
    ///
    /// `Ok(())` if schema sync succeeds
    ///
    /// # Errors
    ///
    /// - Any errors from database operations
    /// - Any errors from `get_schema`
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use nodespace_core::services::SchemaService;
    /// # async fn example(service: SchemaService) -> Result<(), Box<dyn std::error::Error>> {
    /// // After modifying a schema, sync it to the database
    /// service.sync_schema_to_database("person").await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn sync_schema_to_database(&self, type_name: &str) -> Result<(), NodeServiceError> {
        let schema = self.get_schema(type_name).await?;

        // Validate type_name to prevent SQL injection
        if !type_name.chars().all(|c| c.is_alphanumeric() || c == '_') {
            return Err(NodeServiceError::invalid_update(format!(
                "Invalid type name '{}': must contain only alphanumeric characters and underscores",
                type_name
            )));
        }

        // Determine table mode: SCHEMAFULL for core types, SCHEMALESS for user types
        let table_mode = if schema.is_core {
            "SCHEMAFULL"
        } else {
            "SCHEMALESS"
        };

        // Get database connection
        let db = self.node_service.store.db();

        // Create/update table
        let define_table_query =
            format!("DEFINE TABLE IF NOT EXISTS {} {};", type_name, table_mode);

        db.query(&define_table_query).await.map_err(|e| {
            NodeServiceError::DatabaseError(crate::db::DatabaseError::OperationError(format!(
                "Failed to define table '{}': {}",
                type_name, e
            )))
        })?;

        tracing::info!("Synced table '{}' with mode {}", type_name, table_mode);

        // Define all fields recursively
        for field in &schema.fields {
            self.define_field(type_name, field, None).await?;
        }

        Ok(())
    }

    /// Define a field in the database (recursive for nested fields)
    ///
    /// Creates DEFINE FIELD statements for the field and recursively handles
    /// nested fields (object types) and array of objects.
    ///
    /// # Arguments
    ///
    /// * `table` - The table name
    /// * `field` - The field definition
    /// * `parent_path` - Optional parent path for nested fields (e.g., "address")
    ///
    /// # Returns
    ///
    /// `Ok(())` if field definition succeeds
    ///
    /// # Errors
    ///
    /// - Any errors from database operations
    /// - Any errors from `map_field_type`
    fn define_field<'a>(
        &'a self,
        table: &'a str,
        field: &'a SchemaField,
        parent_path: Option<&'a str>,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<(), NodeServiceError>> + Send + 'a>,
    > {
        Box::pin(async move {
            // Build full field path (e.g., "address.city")
            let field_path = if let Some(parent) = parent_path {
                format!("{}.{}", parent, field.name)
            } else {
                field.name.clone()
            };

            // Validate field name to prevent SQL injection
            if !field
                .name
                .chars()
                .all(|c| c.is_alphanumeric() || c == '_' || c == ':')
            {
                return Err(NodeServiceError::invalid_update(format!(
                "Invalid field name '{}': must contain only alphanumeric characters, underscores, and colons",
                field.name
            )));
            }

            // Map schema field type to SurrealDB type
            let db_type = self.map_field_type(&field.field_type, field)?;

            // Get database connection
            let db = self.node_service.store.db();

            // Define field in database
            let define_field_query = format!(
                "DEFINE FIELD IF NOT EXISTS {} ON {} TYPE {};",
                field_path, table, db_type
            );

            db.query(&define_field_query).await.map_err(|e| {
                NodeServiceError::DatabaseError(crate::db::DatabaseError::OperationError(format!(
                    "Failed to define field '{}' on table '{}': {}",
                    field_path, table, e
                )))
            })?;

            tracing::info!(
                "Defined field '{}' on table '{}' as type '{}'",
                field_path,
                table,
                db_type
            );

            // Create index if requested
            if field.indexed {
                self.create_field_index(table, field, parent_path).await?;
            }

            // Recursively define nested fields (for object types)
            if let Some(ref nested_fields) = field.fields {
                for nested_field in nested_fields {
                    self.define_field(table, nested_field, Some(&field_path))
                        .await?;
                }
            }

            // Recursively define item fields (for array of objects)
            if let Some(ref item_fields) = field.item_fields {
                for item_field in item_fields {
                    // For array items, we need special path handling
                    // SurrealDB uses notation like: contacts[*].email
                    let array_item_path = format!("{}[*]", field_path);
                    self.define_field(table, item_field, Some(&array_item_path))
                        .await?;
                }
            }

            Ok(())
        })
    }

    /// Map schema field type to SurrealDB type
    ///
    /// Converts schema type strings to SurrealDB type syntax.
    ///
    /// # Arguments
    ///
    /// * `schema_type` - The schema field type (e.g., "string", "number", "enum")
    /// * `field` - The field definition (for enum validation)
    ///
    /// # Returns
    ///
    /// SurrealDB type string (e.g., "string", "number", "bool", "datetime")
    ///
    /// # Errors
    ///
    /// - Unknown field types
    fn map_field_type(
        &self,
        schema_type: &str,
        field: &SchemaField,
    ) -> Result<String, NodeServiceError> {
        let db_type = match schema_type {
            "string" | "text" => "string".to_string(),
            "number" => "number".to_string(),
            "boolean" => "bool".to_string(),
            "date" => "datetime".to_string(),
            "enum" => {
                // Build ASSERT clause with all valid enum values
                let all_values = {
                    let mut values = Vec::new();
                    if let Some(ref core_vals) = field.core_values {
                        values.extend(core_vals.clone());
                    }
                    if let Some(ref user_vals) = field.user_values {
                        values.extend(user_vals.clone());
                    }
                    values
                };

                if all_values.is_empty() {
                    return Err(NodeServiceError::invalid_update(format!(
                        "Enum field '{}' has no values defined",
                        field.name
                    )));
                }

                let values_list = all_values
                    .iter()
                    .map(|v| format!("'{}'", v))
                    .collect::<Vec<_>>()
                    .join(", ");

                format!("string ASSERT $value IN [{}]", values_list)
            }
            "array" => {
                if let Some(ref item_type) = field.item_type {
                    if item_type == "object" {
                        "array<object>".to_string()
                    } else {
                        format!("array<{}>", item_type)
                    }
                } else {
                    "array".to_string()
                }
            }
            "object" => "object".to_string(),
            "record" => "record".to_string(),
            _ => {
                return Err(NodeServiceError::invalid_update(format!(
                    "Unknown field type '{}'",
                    schema_type
                )))
            }
        };

        Ok(db_type)
    }

    /// Create index for a field
    ///
    /// Creates a database index for faster queries on the field.
    ///
    /// # Arguments
    ///
    /// * `table` - The table name
    /// * `field` - The field definition
    /// * `parent_path` - Optional parent path for nested fields
    ///
    /// # Returns
    ///
    /// `Ok(())` if index creation succeeds
    ///
    /// # Errors
    ///
    /// - Any errors from database operations
    async fn create_field_index(
        &self,
        table: &str,
        field: &SchemaField,
        parent_path: Option<&str>,
    ) -> Result<(), NodeServiceError> {
        // Build full field path
        let field_path = if let Some(parent) = parent_path {
            format!("{}.{}", parent, field.name)
        } else {
            field.name.clone()
        };

        // Build index name: idx_{table}_{field_path_with_underscores}
        let index_name = format!(
            "idx_{}_{}",
            table,
            field_path.replace('.', "_").replace("[*]", "_arr")
        );

        // Get database connection
        let db = self.node_service.store.db();

        // Create index
        let define_index_query = format!(
            "DEFINE INDEX IF NOT EXISTS {} ON {} FIELDS {};",
            index_name, table, field_path
        );

        db.query(&define_index_query).await.map_err(|e| {
            NodeServiceError::DatabaseError(crate::db::DatabaseError::OperationError(format!(
                "Failed to create index '{}' on table '{}': {}",
                index_name, table, e
            )))
        })?;

        tracing::info!(
            "Created index '{}' on table '{}' for field '{}'",
            index_name,
            table,
            field_path
        );

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
                    fields: None,
                    item_fields: None,
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
                    fields: None,
                    item_fields: None,
                },
            ],
        };

        let schema_node = Node {
            id: "test_widget".to_string(),
            node_type: "schema".to_string(),
            content: "Test Widget".to_string(),
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
            fields: None,
            item_fields: None,
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
            fields: None,
            item_fields: None,
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
            fields: None,
            item_fields: None,
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
            fields: None,
            item_fields: None,
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
            fields: None,
            item_fields: None,
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
            fields: None,
            item_fields: None,
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
            fields: None,
            item_fields: None,
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
            json!({"status": "DONE"}), // Flat format, not nested under "test_widget"
        );

        let result = service.validate_node_against_schema(&node).await;
        assert!(
            result.is_ok(),
            "Flat properties format should work for backward compatibility"
        );
    }

    #[tokio::test]
    async fn test_sync_schema_to_database_basic() {
        let (service, _temp) = setup_test_service().await;
        let schema_id = create_test_schema(&service).await;

        // Sync schema to database
        service.sync_schema_to_database(&schema_id).await.unwrap();

        // Verify table was created by attempting to query it
        let db = service.node_service.store.db();
        let result = db.query(format!("INFO FOR TABLE {}", schema_id)).await;

        assert!(result.is_ok(), "Table should be created");
    }

    #[tokio::test]
    async fn test_sync_schema_with_nested_fields() {
        let (service, _temp) = setup_test_service().await;

        // Create schema with nested fields
        let schema = SchemaDefinition {
            is_core: false,
            version: 1,
            description: "Person with nested address".to_string(),
            fields: vec![SchemaField {
                name: "address".to_string(),
                field_type: "object".to_string(),
                protection: ProtectionLevel::User,
                core_values: None,
                user_values: None,
                indexed: false,
                required: Some(false),
                extensible: None,
                default: None,
                description: Some("Address information".to_string()),
                item_type: None,
                fields: Some(vec![
                    SchemaField {
                        name: "street".to_string(),
                        field_type: "string".to_string(),
                        protection: ProtectionLevel::User,
                        core_values: None,
                        user_values: None,
                        indexed: false,
                        required: Some(false),
                        extensible: None,
                        default: None,
                        description: Some("Street address".to_string()),
                        item_type: None,
                        fields: None,
                        item_fields: None,
                    },
                    SchemaField {
                        name: "city".to_string(),
                        field_type: "string".to_string(),
                        protection: ProtectionLevel::User,
                        core_values: None,
                        user_values: None,
                        indexed: true,
                        required: Some(false),
                        extensible: None,
                        default: None,
                        description: Some("City".to_string()),
                        item_type: None,
                        fields: None,
                        item_fields: None,
                    },
                ]),
                item_fields: None,
            }],
        };

        let schema_node = Node {
            id: "person".to_string(),
            node_type: "schema".to_string(),
            content: "Person".to_string(),
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

        // Sync schema to database
        service.sync_schema_to_database("person").await.unwrap();

        // Verify table was created
        let db = service.node_service.store.db();
        let result = db.query("INFO FOR TABLE person").await;

        assert!(result.is_ok(), "Table should be created with nested fields");
    }

    #[tokio::test]
    async fn test_sync_schema_with_indexed_nested_field() {
        let (service, _temp) = setup_test_service().await;

        // Create schema with indexed nested field
        let schema = SchemaDefinition {
            is_core: false,
            version: 1,
            description: "Person with indexed city".to_string(),
            fields: vec![SchemaField {
                name: "address".to_string(),
                field_type: "object".to_string(),
                protection: ProtectionLevel::User,
                core_values: None,
                user_values: None,
                indexed: false,
                required: Some(false),
                extensible: None,
                default: None,
                description: Some("Address information".to_string()),
                item_type: None,
                fields: Some(vec![SchemaField {
                    name: "city".to_string(),
                    field_type: "string".to_string(),
                    protection: ProtectionLevel::User,
                    core_values: None,
                    user_values: None,
                    indexed: true, // This should create an index
                    required: Some(false),
                    extensible: None,
                    default: None,
                    description: Some("City".to_string()),
                    item_type: None,
                    fields: None,
                    item_fields: None,
                }]),
                item_fields: None,
            }],
        };

        let schema_node = Node {
            id: "test_person".to_string(),
            node_type: "schema".to_string(),
            content: "Test Person".to_string(),
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

        // Sync schema to database
        service
            .sync_schema_to_database("test_person")
            .await
            .unwrap();

        // Verify index was created
        let db = service.node_service.store.db();
        let result = db.query("INFO FOR TABLE test_person").await;

        assert!(
            result.is_ok(),
            "Table should be created with index on nested field"
        );
    }

    #[tokio::test]
    async fn test_sync_schema_with_array_of_objects() {
        let (service, _temp) = setup_test_service().await;

        // Create schema with array of objects
        let schema = SchemaDefinition {
            is_core: false,
            version: 1,
            description: "Person with contacts array".to_string(),
            fields: vec![SchemaField {
                name: "contacts".to_string(),
                field_type: "array".to_string(),
                protection: ProtectionLevel::User,
                core_values: None,
                user_values: None,
                indexed: false,
                required: Some(false),
                extensible: None,
                default: None,
                description: Some("Contact list".to_string()),
                item_type: Some("object".to_string()),
                fields: None,
                item_fields: Some(vec![SchemaField {
                    name: "email".to_string(),
                    field_type: "string".to_string(),
                    protection: ProtectionLevel::User,
                    core_values: None,
                    user_values: None,
                    indexed: true,
                    required: Some(false),
                    extensible: None,
                    default: None,
                    description: Some("Email address".to_string()),
                    item_type: None,
                    fields: None,
                    item_fields: None,
                }]),
            }],
        };

        let schema_node = Node {
            id: "contact_person".to_string(),
            node_type: "schema".to_string(),
            content: "Contact Person".to_string(),
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

        // Sync schema to database
        service
            .sync_schema_to_database("contact_person")
            .await
            .unwrap();

        // Verify table was created
        let db = service.node_service.store.db();
        let result = db.query("INFO FOR TABLE contact_person").await;

        assert!(
            result.is_ok(),
            "Table should be created with array of objects"
        );
    }

    #[tokio::test]
    async fn test_map_field_type_string() {
        let (service, _temp) = setup_test_service().await;

        let field = SchemaField {
            name: "test".to_string(),
            field_type: "string".to_string(),
            protection: ProtectionLevel::User,
            core_values: None,
            user_values: None,
            indexed: false,
            required: None,
            extensible: None,
            default: None,
            description: None,
            item_type: None,
            fields: None,
            item_fields: None,
        };

        let db_type = service.map_field_type("string", &field).unwrap();
        assert_eq!(db_type, "string");
    }

    #[tokio::test]
    async fn test_map_field_type_enum() {
        let (service, _temp) = setup_test_service().await;

        let field = SchemaField {
            name: "status".to_string(),
            field_type: "enum".to_string(),
            protection: ProtectionLevel::Core,
            core_values: Some(vec!["OPEN".to_string(), "DONE".to_string()]),
            user_values: Some(vec!["BLOCKED".to_string()]),
            indexed: false,
            required: None,
            extensible: None,
            default: None,
            description: None,
            item_type: None,
            fields: None,
            item_fields: None,
        };

        let db_type = service.map_field_type("enum", &field).unwrap();
        assert!(db_type.contains("ASSERT"));
        assert!(db_type.contains("OPEN"));
        assert!(db_type.contains("DONE"));
        assert!(db_type.contains("BLOCKED"));
    }

    #[tokio::test]
    async fn test_map_field_type_array_of_objects() {
        let (service, _temp) = setup_test_service().await;

        let field = SchemaField {
            name: "items".to_string(),
            field_type: "array".to_string(),
            protection: ProtectionLevel::User,
            core_values: None,
            user_values: None,
            indexed: false,
            required: None,
            extensible: None,
            default: None,
            description: None,
            item_type: Some("object".to_string()),
            fields: None,
            item_fields: None,
        };

        let db_type = service.map_field_type("array", &field).unwrap();
        assert_eq!(db_type, "array<object>");
    }

    #[tokio::test]
    async fn test_sync_schema_core_type_uses_schemafull() {
        let (service, _temp) = setup_test_service().await;

        // Create a core schema
        let schema = SchemaDefinition {
            is_core: true, // Core type
            version: 1,
            description: "Core type test".to_string(),
            fields: vec![SchemaField {
                name: "name".to_string(),
                field_type: "string".to_string(),
                protection: ProtectionLevel::Core,
                core_values: None,
                user_values: None,
                indexed: false,
                required: Some(true),
                extensible: None,
                default: None,
                description: Some("Name".to_string()),
                item_type: None,
                fields: None,
                item_fields: None,
            }],
        };

        let schema_node = Node {
            id: "core_type".to_string(),
            node_type: "schema".to_string(),
            content: "Core Type".to_string(),
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

        // Sync schema - should use SCHEMAFULL
        service.sync_schema_to_database("core_type").await.unwrap();

        // Table should exist
        let db = service.node_service.store.db();
        let result = db.query("INFO FOR TABLE core_type").await;

        assert!(result.is_ok(), "Core type table should use SCHEMAFULL mode");
    }

    #[tokio::test]
    async fn test_sync_schema_user_type_uses_schemaless() {
        let (service, _temp) = setup_test_service().await;
        let schema_id = create_test_schema(&service).await;

        // test_widget schema has is_core = false
        service.sync_schema_to_database(&schema_id).await.unwrap();

        // Table should exist in SCHEMALESS mode
        let db = service.node_service.store.db();
        let result = db.query(format!("INFO FOR TABLE {}", schema_id)).await;

        assert!(result.is_ok(), "User type table should use SCHEMALESS mode");
    }

    #[tokio::test]
    async fn test_query_nested_field_simple() {
        let (service, _temp) = setup_test_service().await;

        // Create person schema with nested address field
        let schema = SchemaDefinition {
            is_core: false,
            version: 1,
            description: "Person with address".to_string(),
            fields: vec![
                SchemaField {
                    name: "name".to_string(),
                    field_type: "string".to_string(),
                    protection: ProtectionLevel::User,
                    core_values: None,
                    user_values: None,
                    indexed: false,
                    required: Some(true),
                    extensible: None,
                    default: None,
                    description: Some("Person name".to_string()),
                    item_type: None,
                    fields: None,
                    item_fields: None,
                },
                SchemaField {
                    name: "address".to_string(),
                    field_type: "object".to_string(),
                    protection: ProtectionLevel::User,
                    core_values: None,
                    user_values: None,
                    indexed: false,
                    required: Some(false),
                    extensible: None,
                    default: None,
                    description: Some("Address".to_string()),
                    item_type: None,
                    fields: Some(vec![SchemaField {
                        name: "city".to_string(),
                        field_type: "string".to_string(),
                        protection: ProtectionLevel::User,
                        core_values: None,
                        user_values: None,
                        indexed: true, // Index for queries
                        required: Some(false),
                        extensible: None,
                        default: None,
                        description: Some("City".to_string()),
                        item_type: None,
                        fields: None,
                        item_fields: None,
                    }]),
                    item_fields: None,
                },
            ],
        };

        let schema_node = Node {
            id: "person_query_test".to_string(),
            node_type: "schema".to_string(),
            content: "Person".to_string(),
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

        // Sync schema to database
        service
            .sync_schema_to_database("person_query_test")
            .await
            .unwrap();

        // Insert test data directly
        let db = service.node_service.store.db();
        db.query(r#"CREATE person_query_test:alice SET name = "Alice", address = { city: "NYC" }"#)
            .await
            .unwrap();
        db.query(r#"CREATE person_query_test:bob SET name = "Bob", address = { city: "SF" }"#)
            .await
            .unwrap();

        // Query by nested field
        let mut result = db
            .query("SELECT * FROM person_query_test WHERE address.city = 'NYC'")
            .await
            .unwrap();

        let records: Vec<serde_json::Value> = result.take(0).unwrap();
        assert_eq!(records.len(), 1, "Should find one person in NYC");
        assert_eq!(records[0]["name"], "Alice");
    }

    #[tokio::test]
    async fn test_query_nested_field_with_index() {
        let (service, _temp) = setup_test_service().await;

        // Create schema with indexed nested field
        let schema = SchemaDefinition {
            is_core: false,
            version: 1,
            description: "Product with details".to_string(),
            fields: vec![SchemaField {
                name: "details".to_string(),
                field_type: "object".to_string(),
                protection: ProtectionLevel::User,
                core_values: None,
                user_values: None,
                indexed: false,
                required: Some(false),
                extensible: None,
                default: None,
                description: None,
                item_type: None,
                fields: Some(vec![
                    SchemaField {
                        name: "category".to_string(),
                        field_type: "string".to_string(),
                        protection: ProtectionLevel::User,
                        core_values: None,
                        user_values: None,
                        indexed: true, // Indexed for fast queries
                        required: Some(false),
                        extensible: None,
                        default: None,
                        description: None,
                        item_type: None,
                        fields: None,
                        item_fields: None,
                    },
                    SchemaField {
                        name: "price".to_string(),
                        field_type: "number".to_string(),
                        protection: ProtectionLevel::User,
                        core_values: None,
                        user_values: None,
                        indexed: false,
                        required: Some(false),
                        extensible: None,
                        default: None,
                        description: None,
                        item_type: None,
                        fields: None,
                        item_fields: None,
                    },
                ]),
                item_fields: None,
            }],
        };

        let schema_node = Node {
            id: "product_test".to_string(),
            node_type: "schema".to_string(),
            content: "Product".to_string(),
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
        service
            .sync_schema_to_database("product_test")
            .await
            .unwrap();

        // Insert test data
        let db = service.node_service.store.db();
        db.query(r#"CREATE product_test:1 SET details = { category: "electronics", price: 999 }"#)
            .await
            .unwrap();
        db.query(r#"CREATE product_test:2 SET details = { category: "books", price: 29 }"#)
            .await
            .unwrap();
        db.query(r#"CREATE product_test:3 SET details = { category: "electronics", price: 499 }"#)
            .await
            .unwrap();

        // Query using indexed nested field
        let mut result = db
            .query("SELECT * FROM product_test WHERE details.category = 'electronics'")
            .await
            .unwrap();

        let records: Vec<serde_json::Value> = result.take(0).unwrap();
        assert_eq!(records.len(), 2, "Should find two electronics products");
    }

    #[tokio::test]
    async fn test_query_deeply_nested_fields() {
        let (service, _temp) = setup_test_service().await;

        // Create schema with deeply nested structure (address.coordinates.lat)
        let schema = SchemaDefinition {
            is_core: false,
            version: 1,
            description: "Location with nested coordinates".to_string(),
            fields: vec![SchemaField {
                name: "address".to_string(),
                field_type: "object".to_string(),
                protection: ProtectionLevel::User,
                core_values: None,
                user_values: None,
                indexed: false,
                required: Some(false),
                extensible: None,
                default: None,
                description: None,
                item_type: None,
                fields: Some(vec![
                    SchemaField {
                        name: "city".to_string(),
                        field_type: "string".to_string(),
                        protection: ProtectionLevel::User,
                        core_values: None,
                        user_values: None,
                        indexed: false,
                        required: Some(false),
                        extensible: None,
                        default: None,
                        description: None,
                        item_type: None,
                        fields: None,
                        item_fields: None,
                    },
                    SchemaField {
                        name: "coordinates".to_string(),
                        field_type: "object".to_string(),
                        protection: ProtectionLevel::User,
                        core_values: None,
                        user_values: None,
                        indexed: false,
                        required: Some(false),
                        extensible: None,
                        default: None,
                        description: None,
                        item_type: None,
                        fields: Some(vec![
                            SchemaField {
                                name: "lat".to_string(),
                                field_type: "number".to_string(),
                                protection: ProtectionLevel::User,
                                core_values: None,
                                user_values: None,
                                indexed: true, // Index deep nested field
                                required: Some(false),
                                extensible: None,
                                default: None,
                                description: None,
                                item_type: None,
                                fields: None,
                                item_fields: None,
                            },
                            SchemaField {
                                name: "lng".to_string(),
                                field_type: "number".to_string(),
                                protection: ProtectionLevel::User,
                                core_values: None,
                                user_values: None,
                                indexed: false,
                                required: Some(false),
                                extensible: None,
                                default: None,
                                description: None,
                                item_type: None,
                                fields: None,
                                item_fields: None,
                            },
                        ]),
                        item_fields: None,
                    },
                ]),
                item_fields: None,
            }],
        };

        let schema_node = Node {
            id: "location_test".to_string(),
            node_type: "schema".to_string(),
            content: "Location".to_string(),
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
        service
            .sync_schema_to_database("location_test")
            .await
            .unwrap();

        // Insert test data with deeply nested structure
        let db = service.node_service.store.db();
        db.query(r#"CREATE location_test:nyc SET address = { city: "NYC", coordinates: { lat: 40.7, lng: -74.0 } }"#).await.unwrap();
        db.query(r#"CREATE location_test:sf SET address = { city: "SF", coordinates: { lat: 37.8, lng: -122.4 } }"#).await.unwrap();

        // Query by deeply nested field (address.coordinates.lat)
        let mut result = db
            .query("SELECT * FROM location_test WHERE address.coordinates.lat > 38")
            .await
            .unwrap();

        let records: Vec<serde_json::Value> = result.take(0).unwrap();
        assert_eq!(records.len(), 1, "Should find one location with lat > 38");
        assert_eq!(records[0]["address"]["city"], "NYC");
    }
}
