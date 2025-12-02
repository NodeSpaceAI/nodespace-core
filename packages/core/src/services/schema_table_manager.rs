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
use crate::models::schema::{EdgeField, SchemaField, SchemaRelationship};
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

        // Use SCHEMAFULL with FLEXIBLE fields to allow user extensions
        // This enforces defined fields while accepting additional properties
        let table_mode = "SCHEMAFULL";

        // DEFINE TABLE statement
        statements.push(format!(
            "DEFINE TABLE IF NOT EXISTS {} {};",
            type_name, table_mode
        ));

        // Define reverse link to hub node (required for hub-spoke architecture)
        statements.push(format!(
            "DEFINE FIELD IF NOT EXISTS node ON TABLE {} TYPE option<record>;",
            type_name
        ));

        // Generate field definitions from schema
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

        // Add DEFINE FIELD statement with FLEXIBLE to allow extra properties
        // FLEXIBLE allows the field to accept any valid JSON value beyond the base type
        statements.push(format!(
            "DEFINE FIELD IF NOT EXISTS {} ON TABLE {} FLEXIBLE TYPE {};",
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
                "DEFINE INDEX IF NOT EXISTS {} ON TABLE {} COLUMNS {};",
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

        // Use SCHEMAFULL with FLEXIBLE fields to allow user extensions
        // This enforces defined fields while accepting additional properties
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

        // Define reverse link to hub node (required for hub-spoke architecture)
        let node_field_query = format!(
            "DEFINE FIELD IF NOT EXISTS node ON TABLE {} TYPE option<record>;",
            type_name
        );
        let mut response = db.query(&node_field_query).await.map_err(|e| {
            NodeServiceError::DatabaseError(crate::db::DatabaseError::OperationError(format!(
                "Failed to define node field on table '{}': {}",
                type_name, e
            )))
        })?;
        let _: Result<Vec<serde_json::Value>, _> = response.take(0);

        // Define all fields recursively from schema
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
        let base_type = match schema_type {
            "string" | "text" | "enum" => "string".to_string(),
            "number" => "number".to_string(),
            "boolean" => "bool".to_string(),
            "date" => "datetime".to_string(),
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

        // Wrap in option<> unless required
        // Required fields have defaults, optional fields are nullable
        let db_type = if field.required.unwrap_or(false) {
            // Required fields need DEFAULT value
            if let Some(ref default) = field.default {
                let default_val = match default {
                    serde_json::Value::String(s) => format!("'{}'", s),
                    serde_json::Value::Number(n) => n.to_string(),
                    serde_json::Value::Bool(b) => b.to_string(),
                    _ => "''".to_string(),
                };
                format!("{} DEFAULT {}", base_type, default_val)
            } else {
                base_type
            }
        } else {
            // Optional fields are nullable
            format!("option<{}>", base_type)
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

    // =========================================================================
    // Relationship DDL Generation (Issue #703)
    // =========================================================================

    /// Generate DDL statements for relationship edge tables
    ///
    /// Creates edge tables for each relationship defined in the schema.
    /// Edge tables use SurrealDB's `TYPE RELATION` for graph queries.
    ///
    /// # Arguments
    ///
    /// * `source_type` - The source node type (e.g., "invoice")
    /// * `relationships` - The relationships to generate edge tables for
    ///
    /// # Returns
    ///
    /// A vector of DDL statements for all edge tables
    ///
    /// # Generated DDL Pattern
    ///
    /// For each relationship, generates:
    /// 1. `DEFINE TABLE ... TYPE RELATION IN {source} OUT {target}`
    /// 2. Core tracking fields (created_at, version)
    /// 3. User-defined edge fields
    /// 4. Indexes (in, out, unique, and user-defined field indexes)
    ///
    /// # Example
    ///
    /// ```ignore
    /// // For invoice.billed_to -> customer relationship:
    /// // DEFINE TABLE invoice_billed_to_customer SCHEMAFULL TYPE RELATION IN invoice OUT customer;
    /// // DEFINE FIELD created_at ON TABLE invoice_billed_to_customer TYPE datetime DEFAULT time::now();
    /// // DEFINE FIELD version ON TABLE invoice_billed_to_customer TYPE int DEFAULT 1;
    /// // DEFINE FIELD billing_date ON TABLE invoice_billed_to_customer TYPE datetime;
    /// // DEFINE INDEX idx_invoice_billed_to_customer_in ON TABLE invoice_billed_to_customer COLUMNS in;
    /// // DEFINE INDEX idx_invoice_billed_to_customer_out ON TABLE invoice_billed_to_customer COLUMNS out;
    /// // DEFINE INDEX idx_invoice_billed_to_customer_unique ON TABLE invoice_billed_to_customer COLUMNS in, out UNIQUE;
    /// ```
    pub fn generate_relationship_ddl_statements(
        &self,
        source_type: &str,
        relationships: &[SchemaRelationship],
    ) -> Result<Vec<String>, NodeServiceError> {
        let mut statements = Vec::new();

        for relationship in relationships {
            // Validate relationship name
            Self::validate_relationship_name(&relationship.name)?;

            // Validate source type
            Self::validate_type_name(source_type)?;

            // Validate target type
            Self::validate_type_name(&relationship.target_type)?;

            // Generate edge table DDL
            let edge_table = relationship.compute_edge_table_name(source_type);
            let edge_ddl = self.generate_edge_table_ddl(&edge_table, source_type, relationship)?;
            statements.extend(edge_ddl);
        }

        Ok(statements)
    }

    /// Generate DDL for a single edge table
    ///
    /// # Parameters
    /// - `edge_table`: Name of the edge table to create
    /// - `_source_type`: Reserved for future type-specific validation (e.g., enforcing
    ///   that only nodes of the source type can be the `in` side of relationships)
    /// - `relationship`: Schema relationship definition with edge fields and constraints
    fn generate_edge_table_ddl(
        &self,
        edge_table: &str,
        _source_type: &str,
        relationship: &SchemaRelationship,
    ) -> Result<Vec<String>, NodeServiceError> {
        let mut statements = Vec::new();

        // 1. Define edge table with RELATION type
        // Note: All nodes are stored in the universal 'node' table (hub-and-spoke architecture)
        // so IN and OUT must reference 'node', not the schema type names
        statements.push(format!(
            "DEFINE TABLE IF NOT EXISTS {} SCHEMAFULL TYPE RELATION IN node OUT node;",
            edge_table
        ));

        // 2. Core tracking fields (always generated)
        statements.push(format!(
            "DEFINE FIELD IF NOT EXISTS created_at ON TABLE {} TYPE datetime DEFAULT time::now();",
            edge_table
        ));
        statements.push(format!(
            "DEFINE FIELD IF NOT EXISTS version ON TABLE {} TYPE int DEFAULT 1;",
            edge_table
        ));

        // 3. User-defined edge fields
        if let Some(ref edge_fields) = relationship.edge_fields {
            for field in edge_fields {
                let field_ddl = self.generate_edge_field_ddl(edge_table, field)?;
                statements.extend(field_ddl);
            }
        }

        // 4. Core indexes (always generated)
        // Index on 'in' for forward traversal queries
        statements.push(format!(
            "DEFINE INDEX IF NOT EXISTS idx_{}_in ON TABLE {} COLUMNS in;",
            edge_table, edge_table
        ));

        // Index on 'out' for reverse traversal queries
        statements.push(format!(
            "DEFINE INDEX IF NOT EXISTS idx_{}_out ON TABLE {} COLUMNS out;",
            edge_table, edge_table
        ));

        // Unique index on (in, out) to prevent duplicate edges
        statements.push(format!(
            "DEFINE INDEX IF NOT EXISTS idx_{}_unique ON TABLE {} COLUMNS in, out UNIQUE;",
            edge_table, edge_table
        ));

        Ok(statements)
    }

    /// Generate DDL for a single edge field
    fn generate_edge_field_ddl(
        &self,
        edge_table: &str,
        field: &EdgeField,
    ) -> Result<Vec<String>, NodeServiceError> {
        let mut statements = Vec::new();

        // Validate field name
        if !field.name.chars().all(|c| c.is_alphanumeric() || c == '_') {
            return Err(NodeServiceError::invalid_update(format!(
                "Invalid edge field name '{}': must contain only alphanumeric characters and underscores",
                field.name
            )));
        }

        // Map edge field type to SurrealDB type
        let db_type = self.map_edge_field_type(field)?;

        // Define field
        statements.push(format!(
            "DEFINE FIELD IF NOT EXISTS {} ON TABLE {} TYPE {};",
            field.name, edge_table, db_type
        ));

        // Create index if requested
        if field.indexed.unwrap_or(false) {
            statements.push(format!(
                "DEFINE INDEX IF NOT EXISTS idx_{}_{} ON TABLE {} COLUMNS {};",
                edge_table, field.name, edge_table, field.name
            ));
        }

        Ok(statements)
    }

    /// Map edge field type to SurrealDB type
    ///
    /// Edge fields support a simpler set of types than schema fields:
    /// - string, number, boolean, date, record
    fn map_edge_field_type(&self, field: &EdgeField) -> Result<String, NodeServiceError> {
        let base_type = match field.field_type.as_str() {
            "string" | "text" => "string".to_string(),
            "number" => "number".to_string(),
            "boolean" => "bool".to_string(),
            "date" => "datetime".to_string(),
            "record" => {
                if let Some(ref target) = field.target_type {
                    format!("record<{}>", target)
                } else {
                    "record".to_string()
                }
            }
            _ => {
                return Err(NodeServiceError::invalid_update(format!(
                    "Unknown edge field type '{}'. Supported types: string, number, boolean, date, record",
                    field.field_type
                )))
            }
        };

        // Handle required vs optional
        let db_type = if field.required.unwrap_or(false) {
            // Required fields with default
            if let Some(ref default) = field.default {
                let default_val = match default {
                    serde_json::Value::String(s) => format!("'{}'", s),
                    serde_json::Value::Number(n) => n.to_string(),
                    serde_json::Value::Bool(b) => b.to_string(),
                    _ => "''".to_string(),
                };
                format!("{} DEFAULT {}", base_type, default_val)
            } else {
                base_type
            }
        } else {
            // Optional fields are nullable
            format!("option<{}>", base_type)
        };

        Ok(db_type)
    }

    /// Validate relationship name
    ///
    /// Relationship names must:
    /// - Start with a letter
    /// - Contain only alphanumeric characters, underscores, and hyphens
    /// - Not be a reserved name (has_child, mentions, node, data)
    fn validate_relationship_name(name: &str) -> Result<(), NodeServiceError> {
        // Check reserved names
        const RESERVED_NAMES: [&str; 4] = ["has_child", "mentions", "node", "data"];
        if RESERVED_NAMES.contains(&name) {
            return Err(NodeServiceError::invalid_update(format!(
                "Relationship name '{}' is reserved. Cannot use: has_child, mentions, node, data",
                name
            )));
        }

        // Check pattern: must start with letter, contain only alphanumeric, underscore, hyphen
        let is_valid = !name.is_empty()
            && name.chars().next().unwrap().is_ascii_alphabetic()
            && name
                .chars()
                .all(|c| c.is_alphanumeric() || c == '_' || c == '-');

        if !is_valid {
            return Err(NodeServiceError::invalid_update(format!(
                "Invalid relationship name '{}': must start with a letter and contain only alphanumeric characters, underscores, and hyphens",
                name
            )));
        }

        Ok(())
    }

    /// Validate type name (for source and target types)
    fn validate_type_name(type_name: &str) -> Result<(), NodeServiceError> {
        if !type_name.chars().all(|c| c.is_alphanumeric() || c == '_') {
            return Err(NodeServiceError::invalid_update(format!(
                "Invalid type name '{}': must contain only alphanumeric characters and underscores",
                type_name
            )));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::schema::{RelationshipCardinality, RelationshipDirection};

    // =========================================================================
    // Relationship Name Validation Tests
    // =========================================================================

    #[test]
    fn test_validate_relationship_name_valid() {
        assert!(
            SchemaTableManager::<surrealdb::engine::local::Db>::validate_relationship_name(
                "billed_to"
            )
            .is_ok()
        );
        assert!(
            SchemaTableManager::<surrealdb::engine::local::Db>::validate_relationship_name(
                "assigned-to"
            )
            .is_ok()
        );
        assert!(
            SchemaTableManager::<surrealdb::engine::local::Db>::validate_relationship_name("owns")
                .is_ok()
        );
        assert!(
            SchemaTableManager::<surrealdb::engine::local::Db>::validate_relationship_name(
                "partOf123"
            )
            .is_ok()
        );
    }

    #[test]
    fn test_validate_relationship_name_reserved() {
        assert!(
            SchemaTableManager::<surrealdb::engine::local::Db>::validate_relationship_name(
                "has_child"
            )
            .is_err()
        );
        assert!(
            SchemaTableManager::<surrealdb::engine::local::Db>::validate_relationship_name(
                "mentions"
            )
            .is_err()
        );
        assert!(
            SchemaTableManager::<surrealdb::engine::local::Db>::validate_relationship_name("node")
                .is_err()
        );
        assert!(
            SchemaTableManager::<surrealdb::engine::local::Db>::validate_relationship_name("data")
                .is_err()
        );
    }

    #[test]
    fn test_validate_relationship_name_invalid_pattern() {
        // Must start with letter
        assert!(
            SchemaTableManager::<surrealdb::engine::local::Db>::validate_relationship_name(
                "123_invalid"
            )
            .is_err()
        );
        assert!(
            SchemaTableManager::<surrealdb::engine::local::Db>::validate_relationship_name(
                "_starts_with_underscore"
            )
            .is_err()
        );

        // No special characters
        assert!(
            SchemaTableManager::<surrealdb::engine::local::Db>::validate_relationship_name(
                "has spaces"
            )
            .is_err()
        );
        assert!(
            SchemaTableManager::<surrealdb::engine::local::Db>::validate_relationship_name(
                "has.dot"
            )
            .is_err()
        );

        // Empty string
        assert!(
            SchemaTableManager::<surrealdb::engine::local::Db>::validate_relationship_name("")
                .is_err()
        );
    }

    // =========================================================================
    // Edge Field DDL Generation Tests
    // =========================================================================

    #[test]
    fn test_map_edge_field_type_string() {
        // Create a mock store - we only test the mapping logic
        let manager = create_test_manager();

        let field = EdgeField {
            name: "role".to_string(),
            field_type: "string".to_string(),
            indexed: None,
            required: None,
            default: None,
            target_type: None,
            description: None,
        };

        let result = manager.map_edge_field_type(&field).unwrap();
        assert_eq!(result, "option<string>");
    }

    #[test]
    fn test_map_edge_field_type_required_with_default() {
        let manager = create_test_manager();

        let field = EdgeField {
            name: "status".to_string(),
            field_type: "string".to_string(),
            indexed: None,
            required: Some(true),
            default: Some(serde_json::json!("pending")),
            target_type: None,
            description: None,
        };

        let result = manager.map_edge_field_type(&field).unwrap();
        assert_eq!(result, "string DEFAULT 'pending'");
    }

    #[test]
    fn test_map_edge_field_type_record_with_target() {
        let manager = create_test_manager();

        let field = EdgeField {
            name: "approved_by".to_string(),
            field_type: "record".to_string(),
            indexed: None,
            required: None,
            default: None,
            target_type: Some("person".to_string()),
            description: None,
        };

        let result = manager.map_edge_field_type(&field).unwrap();
        assert_eq!(result, "option<record<person>>");
    }

    #[test]
    fn test_map_edge_field_type_date() {
        let manager = create_test_manager();

        let field = EdgeField {
            name: "created".to_string(),
            field_type: "date".to_string(),
            indexed: None,
            required: Some(true),
            default: None,
            target_type: None,
            description: None,
        };

        let result = manager.map_edge_field_type(&field).unwrap();
        assert_eq!(result, "datetime");
    }

    #[test]
    fn test_map_edge_field_type_unknown() {
        let manager = create_test_manager();

        let field = EdgeField {
            name: "bad".to_string(),
            field_type: "unknown_type".to_string(),
            indexed: None,
            required: None,
            default: None,
            target_type: None,
            description: None,
        };

        assert!(manager.map_edge_field_type(&field).is_err());
    }

    // =========================================================================
    // Edge Table DDL Generation Tests
    // =========================================================================

    #[test]
    fn test_generate_edge_table_ddl_basic() {
        let manager = create_test_manager();

        let relationship = SchemaRelationship {
            name: "billed_to".to_string(),
            target_type: "customer".to_string(),
            direction: RelationshipDirection::Out,
            cardinality: RelationshipCardinality::One,
            required: None,
            reverse_name: None,
            reverse_cardinality: None,
            edge_table: None,
            edge_fields: None,
            description: None,
        };

        let statements = manager
            .generate_edge_table_ddl("invoice_billed_to_customer", "invoice", &relationship)
            .unwrap();

        // Should have: table definition, created_at, version, 3 indexes
        assert!(statements.len() >= 6);

        // Check table definition
        assert!(statements[0].contains("DEFINE TABLE IF NOT EXISTS invoice_billed_to_customer"));
        assert!(statements[0].contains("TYPE RELATION IN invoice OUT customer"));

        // Check tracking fields
        assert!(statements[1].contains("created_at"));
        assert!(statements[2].contains("version"));

        // Check indexes
        let indexes: Vec<_> = statements.iter().filter(|s| s.contains("INDEX")).collect();
        assert_eq!(indexes.len(), 3); // in, out, unique
    }

    #[test]
    fn test_generate_edge_table_ddl_with_fields() {
        let manager = create_test_manager();

        let relationship = SchemaRelationship {
            name: "assigned_to".to_string(),
            target_type: "person".to_string(),
            direction: RelationshipDirection::Out,
            cardinality: RelationshipCardinality::Many,
            required: None,
            reverse_name: None,
            reverse_cardinality: None,
            edge_table: None,
            edge_fields: Some(vec![
                EdgeField {
                    name: "role".to_string(),
                    field_type: "string".to_string(),
                    indexed: Some(true),
                    required: None,
                    default: None,
                    target_type: None,
                    description: None,
                },
                EdgeField {
                    name: "assigned_at".to_string(),
                    field_type: "date".to_string(),
                    indexed: None,
                    required: Some(true),
                    default: None,
                    target_type: None,
                    description: None,
                },
            ]),
            description: None,
        };

        let statements = manager
            .generate_edge_table_ddl("task_assigned_to_person", "task", &relationship)
            .unwrap();

        // Should have edge field definitions
        let role_field: Vec<_> = statements
            .iter()
            .filter(|s| s.contains("FIELD") && s.contains("role"))
            .collect();
        assert_eq!(role_field.len(), 1);
        assert!(role_field[0].contains("option<string>"));

        let assigned_at_field: Vec<_> = statements
            .iter()
            .filter(|s| s.contains("FIELD") && s.contains("assigned_at"))
            .collect();
        assert_eq!(assigned_at_field.len(), 1);
        assert!(assigned_at_field[0].contains("datetime"));

        // Should have index for role (marked as indexed)
        let role_index: Vec<_> = statements
            .iter()
            .filter(|s| s.contains("INDEX") && s.contains("_role"))
            .collect();
        assert_eq!(role_index.len(), 1);
    }

    #[test]
    fn test_generate_relationship_ddl_statements_multiple() {
        let manager = create_test_manager();

        let relationships = vec![
            SchemaRelationship {
                name: "billed_to".to_string(),
                target_type: "customer".to_string(),
                direction: RelationshipDirection::Out,
                cardinality: RelationshipCardinality::One,
                required: None,
                reverse_name: None,
                reverse_cardinality: None,
                edge_table: None,
                edge_fields: None,
                description: None,
            },
            SchemaRelationship {
                name: "shipped_to".to_string(),
                target_type: "address".to_string(),
                direction: RelationshipDirection::Out,
                cardinality: RelationshipCardinality::One,
                required: None,
                reverse_name: None,
                reverse_cardinality: None,
                edge_table: None,
                edge_fields: None,
                description: None,
            },
        ];

        let statements = manager
            .generate_relationship_ddl_statements("invoice", &relationships)
            .unwrap();

        // Should have DDL for both relationships
        assert!(statements
            .iter()
            .any(|s| s.contains("invoice_billed_to_customer")));
        assert!(statements
            .iter()
            .any(|s| s.contains("invoice_shipped_to_address")));
    }

    #[test]
    fn test_generate_relationship_ddl_reserved_name_rejected() {
        let manager = create_test_manager();

        let relationships = vec![SchemaRelationship {
            name: "has_child".to_string(), // Reserved!
            target_type: "node".to_string(),
            direction: RelationshipDirection::Out,
            cardinality: RelationshipCardinality::Many,
            required: None,
            reverse_name: None,
            reverse_cardinality: None,
            edge_table: None,
            edge_fields: None,
            description: None,
        }];

        let result = manager.generate_relationship_ddl_statements("parent", &relationships);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("reserved"));
    }

    #[test]
    fn test_generate_relationship_ddl_custom_edge_table() {
        let manager = create_test_manager();

        let relationships = vec![SchemaRelationship {
            name: "collaborates_with".to_string(),
            target_type: "person".to_string(),
            direction: RelationshipDirection::Out,
            cardinality: RelationshipCardinality::Many,
            required: None,
            reverse_name: None,
            reverse_cardinality: None,
            edge_table: Some("collaborations".to_string()),
            edge_fields: None,
            description: None,
        }];

        let statements = manager
            .generate_relationship_ddl_statements("project", &relationships)
            .unwrap();

        // Should use custom edge table name
        assert!(statements
            .iter()
            .any(|s| s.contains("DEFINE TABLE IF NOT EXISTS collaborations")));
        // Should NOT use auto-generated name
        assert!(!statements
            .iter()
            .any(|s| s.contains("project_collaborates_with_person")));
    }

    // Helper to create a test manager without a real database connection
    fn create_test_manager() -> TestSchemaTableManager {
        TestSchemaTableManager {}
    }

    /// Test-only version of SchemaTableManager that doesn't need a database
    struct TestSchemaTableManager {}

    impl TestSchemaTableManager {
        fn map_edge_field_type(&self, field: &EdgeField) -> Result<String, NodeServiceError> {
            let base_type = match field.field_type.as_str() {
                "string" | "text" => "string".to_string(),
                "number" => "number".to_string(),
                "boolean" => "bool".to_string(),
                "date" => "datetime".to_string(),
                "record" => {
                    if let Some(ref target) = field.target_type {
                        format!("record<{}>", target)
                    } else {
                        "record".to_string()
                    }
                }
                _ => {
                    return Err(NodeServiceError::invalid_update(format!(
                        "Unknown edge field type '{}'",
                        field.field_type
                    )))
                }
            };

            let db_type = if field.required.unwrap_or(false) {
                if let Some(ref default) = field.default {
                    let default_val = match default {
                        serde_json::Value::String(s) => format!("'{}'", s),
                        serde_json::Value::Number(n) => n.to_string(),
                        serde_json::Value::Bool(b) => b.to_string(),
                        _ => "''".to_string(),
                    };
                    format!("{} DEFAULT {}", base_type, default_val)
                } else {
                    base_type
                }
            } else {
                format!("option<{}>", base_type)
            };

            Ok(db_type)
        }

        fn generate_edge_field_ddl(
            &self,
            edge_table: &str,
            field: &EdgeField,
        ) -> Result<Vec<String>, NodeServiceError> {
            let mut statements = Vec::new();

            if !field.name.chars().all(|c| c.is_alphanumeric() || c == '_') {
                return Err(NodeServiceError::invalid_update(format!(
                    "Invalid edge field name '{}'",
                    field.name
                )));
            }

            let db_type = self.map_edge_field_type(field)?;

            statements.push(format!(
                "DEFINE FIELD IF NOT EXISTS {} ON TABLE {} TYPE {};",
                field.name, edge_table, db_type
            ));

            if field.indexed.unwrap_or(false) {
                statements.push(format!(
                    "DEFINE INDEX IF NOT EXISTS idx_{}_{} ON TABLE {} COLUMNS {};",
                    edge_table, field.name, edge_table, field.name
                ));
            }

            Ok(statements)
        }

        fn generate_edge_table_ddl(
            &self,
            edge_table: &str,
            source_type: &str,
            relationship: &SchemaRelationship,
        ) -> Result<Vec<String>, NodeServiceError> {
            let mut statements = Vec::new();

            statements.push(format!(
                "DEFINE TABLE IF NOT EXISTS {} SCHEMAFULL TYPE RELATION IN {} OUT {};",
                edge_table, source_type, relationship.target_type
            ));

            statements.push(format!(
                "DEFINE FIELD IF NOT EXISTS created_at ON TABLE {} TYPE datetime DEFAULT time::now();",
                edge_table
            ));
            statements.push(format!(
                "DEFINE FIELD IF NOT EXISTS version ON TABLE {} TYPE int DEFAULT 1;",
                edge_table
            ));

            if let Some(ref edge_fields) = relationship.edge_fields {
                for field in edge_fields {
                    let field_ddl = self.generate_edge_field_ddl(edge_table, field)?;
                    statements.extend(field_ddl);
                }
            }

            statements.push(format!(
                "DEFINE INDEX IF NOT EXISTS idx_{}_in ON TABLE {} COLUMNS in;",
                edge_table, edge_table
            ));
            statements.push(format!(
                "DEFINE INDEX IF NOT EXISTS idx_{}_out ON TABLE {} COLUMNS out;",
                edge_table, edge_table
            ));
            statements.push(format!(
                "DEFINE INDEX IF NOT EXISTS idx_{}_unique ON TABLE {} COLUMNS in, out UNIQUE;",
                edge_table, edge_table
            ));

            Ok(statements)
        }

        fn generate_relationship_ddl_statements(
            &self,
            source_type: &str,
            relationships: &[SchemaRelationship],
        ) -> Result<Vec<String>, NodeServiceError> {
            let mut statements = Vec::new();

            for relationship in relationships {
                SchemaTableManager::<surrealdb::engine::local::Db>::validate_relationship_name(
                    &relationship.name,
                )?;
                SchemaTableManager::<surrealdb::engine::local::Db>::validate_type_name(
                    source_type,
                )?;
                SchemaTableManager::<surrealdb::engine::local::Db>::validate_type_name(
                    &relationship.target_type,
                )?;

                let edge_table = relationship.compute_edge_table_name(source_type);
                let edge_ddl =
                    self.generate_edge_table_ddl(&edge_table, source_type, relationship)?;
                statements.extend(edge_ddl);
            }

            Ok(statements)
        }
    }
}
