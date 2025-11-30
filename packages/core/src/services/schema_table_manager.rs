//! Schema Table Manager
//!
//! Handles DDL (Data Definition Language) generation for schema-defined tables.
//! Extracted from SchemaService to support the simplified schema architecture
//! where schemas use generic CRUD operations.
//!
//! ## Responsibilities
//!
//! - Creating and defining tables based on schema fields
//! - Defining fields with proper SurrealDB types
//! - Managing indexes for optimized queries
//! - Handling nested fields and array structures
//! - Generating DDL statements for atomic transactions (Issue #690)
//!
//! ## Atomic Schema Updates
//!
//! When updating a schema node via the generic CRUD API (update_node), both the
//! node data and the SurrealDB table definitions must change atomically. The
//! `generate_ddl_statements` method produces DDL statements without executing them,
//! allowing NodeService to wrap both the node update and DDL execution in a
//! single transaction.
//!
//! ## Example Usage
//!
//! ```ignore
//! use nodespace_core::services::SchemaTableManager;
//! use nodespace_core::db::SurrealStore;
//! use std::sync::Arc;
//!
//! #[tokio::main]
//! async fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     let store = Arc::new(SurrealStore::new("./test.db").await?);
//!     let table_manager = SchemaTableManager::new(store);
//!
//!     // Sync schema fields to database tables
//!     let fields = vec![]; // SchemaField instances
//!     table_manager.sync_schema_to_database("person", &fields).await?;
//!     Ok(())
//! }
//! ```

use crate::db::SurrealStore;
use crate::models::schema::SchemaField;
use crate::services::NodeServiceError;
use std::sync::Arc;

/// Manages database table definitions for schema nodes
///
/// Handles creating and syncing database tables based on schema definitions.
/// This includes:
/// - Creating spoke tables for schema types
/// - Defining fields with proper SurrealDB types
/// - Creating indexes for optimized queries
/// - Managing nested object fields and array structures
pub struct SchemaTableManager<C = surrealdb::engine::local::Db>
where
    C: surrealdb::Connection,
{
    store: Arc<SurrealStore<C>>,
}

impl<C> SchemaTableManager<C>
where
    C: surrealdb::Connection,
{
    /// Create a new SchemaTableManager
    ///
    /// # Arguments
    ///
    /// * `store` - SurrealDB store instance
    pub fn new(store: Arc<SurrealStore<C>>) -> Self {
        Self { store }
    }

    /// Generate DDL statements for a schema without executing them
    ///
    /// This method produces a list of DDL statements (DEFINE TABLE, DEFINE FIELD,
    /// DEFINE INDEX) that can be executed atomically alongside node updates.
    ///
    /// Used by NodeService.update_node to ensure atomic schema updates where
    /// both the schema node data and the SurrealDB table definitions change
    /// together in a single transaction.
    ///
    /// # Arguments
    ///
    /// * `type_name` - The table name (must be alphanumeric + underscores)
    /// * `schema` - The schema definition containing fields and configuration
    ///
    /// # Returns
    ///
    /// A vector of DDL statements to be executed atomically
    ///
    /// # Errors
    ///
    /// - Invalid type name (contains non-alphanumeric characters besides underscores)
    /// - Invalid field names or types
    ///
    /// # Example
    ///
    /// ```ignore
    /// # use nodespace_core::services::SchemaTableManager;
    /// # use nodespace_core::models::schema::SchemaField;
    /// # fn example(manager: &SchemaTableManager, fields: &[SchemaField]) -> Result<(), Box<dyn std::error::Error>> {
    /// let ddl_statements = manager.generate_ddl_statements("person", fields)?;
    /// // Execute these statements in a transaction along with the node update
    /// # Ok(())
    /// # }
    /// ```
    pub fn generate_ddl_statements(
        &self,
        type_name: &str,
        fields: &[SchemaField],
    ) -> Result<Vec<String>, NodeServiceError> {
        // Validate type_name to prevent SQL injection
        if !type_name.chars().all(|c| c.is_alphanumeric() || c == '_') {
            return Err(NodeServiceError::invalid_update(format!(
                "Invalid type name '{}': must contain only alphanumeric characters and underscores",
                type_name
            )));
        }

        let mut statements = Vec::new();

        // Use SCHEMAFULL for all types
        let table_mode = "SCHEMAFULL";

        // DEFINE TABLE statement
        statements.push(format!(
            "DEFINE TABLE IF NOT EXISTS {} {};",
            type_name, table_mode
        ));

        // Generate field definitions
        for field in fields {
            self.generate_field_ddl(type_name, field, None, &mut statements)?;
        }

        Ok(statements)
    }

    /// Generate DDL statements for a field (recursive for nested fields)
    ///
    /// Appends DEFINE FIELD and DEFINE INDEX statements to the provided vector.
    ///
    /// # Arguments
    ///
    /// * `table` - The table name
    /// * `field` - The field definition
    /// * `parent_path` - Optional parent path for nested fields
    /// * `statements` - Vector to append DDL statements to
    fn generate_field_ddl(
        &self,
        table: &str,
        field: &SchemaField,
        parent_path: Option<&str>,
        statements: &mut Vec<String>,
    ) -> Result<(), NodeServiceError> {
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

        // Quote field paths that contain colons
        let quoted_field = if field_path.contains(':') {
            format!("`{}`", field_path)
        } else {
            field_path.clone()
        };

        // Add DEFINE FIELD statement
        statements.push(format!(
            "DEFINE FIELD IF NOT EXISTS {} ON {} TYPE {};",
            quoted_field, table, db_type
        ));

        // Add index if requested
        if field.indexed {
            let index_name = format!(
                "idx_{}_{}",
                table,
                field_path
                    .replace('.', "_")
                    .replace("[*]", "_arr")
                    .replace(':', "_")
            );
            statements.push(format!(
                "DEFINE INDEX IF NOT EXISTS {} ON {} FIELDS {};",
                index_name, table, quoted_field
            ));
        }

        // Recursively handle nested fields (for object types)
        if let Some(ref nested_fields) = field.fields {
            for nested_field in nested_fields {
                self.generate_field_ddl(table, nested_field, Some(&field_path), statements)?;
            }
        }

        // Recursively handle item fields (for array of objects)
        if let Some(ref item_fields) = field.item_fields {
            let array_item_path = format!("{}[*]", field_path);
            for item_field in item_fields {
                self.generate_field_ddl(table, item_field, Some(&array_item_path), statements)?;
            }
        }

        Ok(())
    }

    /// Sync schema fields to database table structure
    ///
    /// Creates or updates the database table to match the schema fields,
    /// including all fields, types, and indexes.
    ///
    /// # Arguments
    ///
    /// * `type_name` - The table name (must be alphanumeric + underscores)
    /// * `fields` - The schema fields to sync
    ///
    /// # Returns
    ///
    /// `Ok(())` if synchronization succeeds
    ///
    /// # Errors
    ///
    /// - Invalid type name (contains non-alphanumeric characters besides underscores)
    /// - Database operation errors
    /// - Field definition errors
    ///
    /// # Example
    ///
    /// ```ignore
    /// # use nodespace_core::services::SchemaTableManager;
    /// # use nodespace_core::models::schema::SchemaField;
    /// # async fn example(manager: SchemaTableManager, fields: Vec<SchemaField>) -> Result<(), Box<dyn std::error::Error>> {
    /// manager.sync_schema_to_database("person", &fields).await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn sync_schema_to_database(
        &self,
        type_name: &str,
        fields: &[SchemaField],
    ) -> Result<(), NodeServiceError> {
        // Validate type_name to prevent SQL injection
        if !type_name.chars().all(|c| c.is_alphanumeric() || c == '_') {
            return Err(NodeServiceError::invalid_update(format!(
                "Invalid type name '{}': must contain only alphanumeric characters and underscores",
                type_name
            )));
        }

        // Use SCHEMAFULL for all types (core and user-defined)
        // SCHEMAFULL with FLEXIBLE fields allows enforcing core fields while accepting user-defined fields
        let table_mode = "SCHEMAFULL";

        // Get database connection
        let db = self.store.db();

        // Create/update table
        let define_table_query =
            format!("DEFINE TABLE IF NOT EXISTS {} {};", type_name, table_mode);

        let mut response = db.query(&define_table_query).await.map_err(|e| {
            NodeServiceError::DatabaseError(crate::db::DatabaseError::OperationError(format!(
                "Failed to define table '{}': {}",
                type_name, e
            )))
        })?;

        // Consume the response to ensure the query is fully executed
        let _: Result<Vec<serde_json::Value>, _> = response.take(0);

        tracing::info!("Synced table '{}' with mode {}", type_name, table_mode);

        // Define all fields recursively
        for field in fields {
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
    /// - Invalid field name (contains invalid characters)
    /// - Database operation errors
    /// - Any errors from `map_field_type`
    ///
    /// # Notes
    ///
    /// This method is recursive and uses `Box::pin` to handle async recursion.
    /// It processes nested object fields and array item fields automatically.
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
            let db = self.store.db();

            // Define field in database
            // Fields with colons in the name need backtick quoting
            let quoted_field = if field_path.contains(':') {
                format!("`{}`", field_path)
            } else {
                field_path.clone()
            };
            let define_field_query = format!(
                "DEFINE FIELD IF NOT EXISTS {} ON {} TYPE {};",
                quoted_field, table, db_type
            );

            let mut response = db.query(&define_field_query).await.map_err(|e| {
                NodeServiceError::DatabaseError(crate::db::DatabaseError::OperationError(format!(
                    "Failed to define field '{}' on table '{}': {}",
                    field_path, table, e
                )))
            })?;

            // Consume the response to ensure the query is fully executed
            let _: Result<Vec<serde_json::Value>, _> = response.take(0);

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
    /// - Enum fields with no values defined
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

                // Extract just the value strings for DDL (not labels)
                let values_list = all_values
                    .iter()
                    .map(|ev| format!("'{}'", ev.value))
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
    /// - Database operation errors
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
            field_path
                .replace('.', "_")
                .replace("[*]", "_arr")
                .replace(':', "_")
        );

        // Get database connection
        let db = self.store.db();

        // Quote field paths that contain colons
        let quoted_field = if field_path.contains(':') {
            format!("`{}`", field_path)
        } else {
            field_path.clone()
        };

        // Create index
        let define_index_query = format!(
            "DEFINE INDEX IF NOT EXISTS {} ON {} FIELDS {};",
            index_name, table, quoted_field
        );

        let mut response = db.query(&define_index_query).await.map_err(|e| {
            NodeServiceError::DatabaseError(crate::db::DatabaseError::OperationError(format!(
                "Failed to create index '{}' on table '{}': {}",
                index_name, table, e
            )))
        })?;

        // Consume the response to ensure the query is fully executed
        let _: Result<Vec<serde_json::Value>, _> = response.take(0);

        tracing::info!(
            "Created index '{}' on table '{}' for field '{}'",
            index_name,
            table,
            field_path
        );

        Ok(())
    }
}
