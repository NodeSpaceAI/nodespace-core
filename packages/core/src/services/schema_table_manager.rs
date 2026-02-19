//! Schema Table Manager
//!
//! Handles DDL (Data Definition Language) generation for relationship edge tables.
//!
//! ## Universal Graph Architecture (Issue #783)
//!
//! With Universal Graph Architecture, all node properties are stored in the
//! `node.properties` field. This module only generates DDL for relationship edge tables.
//!
//! ## Responsibilities
//!
//! - Generating DDL statements for relationship edge tables
//! - Mapping edge field types to proper SurrealDB types
//! - Generating index definitions for efficient graph traversal
//!
//! ## Example Usage
//!
//! ```ignore
//! use nodespace_core::services::SchemaTableManager;
//!
//! let table_manager = SchemaTableManager::new();
//!
//! // Generate DDL statements for relationship edge tables
//! let relationships = vec![]; // SchemaRelationship instances
//! let ddl_statements = table_manager.generate_relationship_ddl_statements("invoice", &relationships)?;
//! // Execute ddl_statements in a transaction with the node update
//! ```

use crate::models::schema::{EdgeField, SchemaRelationship};
use crate::services::NodeServiceError;

/// Pure DDL generator for relationship edge tables
///
/// Universal Graph Architecture (Issue #783): This struct only generates DDL
/// for relationship edge tables. Spoke tables are no longer used - all node
/// properties are stored in `node.properties`.
///
/// ## Design
///
/// `SchemaTableManager` is stateless and does not hold database connections.
/// DDL execution is the responsibility of the caller (typically `NodeService`),
/// which wraps DDL execution in transactions for atomicity.
#[derive(Default)]
pub struct SchemaTableManager;

impl SchemaTableManager {
    /// Create a new SchemaTableManager
    pub fn new() -> Self {
        Self
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

            // Validate target type (skip when None — untyped relationship)
            if let Some(target_type) = &relationship.target_type {
                Self::validate_type_name(target_type)?;
            }

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
        // Universal Graph Architecture (Issue #783): All nodes are stored in the 'node' table
        // so IN and OUT reference 'node', not schema type names
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
        assert!(SchemaTableManager::validate_relationship_name("billed_to").is_ok());
        assert!(SchemaTableManager::validate_relationship_name("assigned-to").is_ok());
        assert!(SchemaTableManager::validate_relationship_name("owns").is_ok());
        assert!(SchemaTableManager::validate_relationship_name("partOf123").is_ok());
    }

    #[test]
    fn test_validate_relationship_name_reserved() {
        assert!(SchemaTableManager::validate_relationship_name("has_child").is_err());
        assert!(SchemaTableManager::validate_relationship_name("mentions").is_err());
        assert!(SchemaTableManager::validate_relationship_name("node").is_err());
        assert!(SchemaTableManager::validate_relationship_name("data").is_err());
    }

    #[test]
    fn test_validate_relationship_name_invalid_pattern() {
        // Must start with letter
        assert!(SchemaTableManager::validate_relationship_name("123_invalid").is_err());
        assert!(SchemaTableManager::validate_relationship_name("_starts_with_underscore").is_err());

        // No special characters
        assert!(SchemaTableManager::validate_relationship_name("has spaces").is_err());
        assert!(SchemaTableManager::validate_relationship_name("has.dot").is_err());

        // Empty string
        assert!(SchemaTableManager::validate_relationship_name("").is_err());
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
            target_type: Some("customer".to_string()),
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

        // Check table definition - Universal Graph Architecture uses 'node' for IN/OUT
        assert!(statements[0].contains("DEFINE TABLE IF NOT EXISTS invoice_billed_to_customer"));
        assert!(statements[0].contains("TYPE RELATION IN node OUT node"));

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
            target_type: Some("person".to_string()),
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
                target_type: Some("customer".to_string()),
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
                target_type: Some("address".to_string()),
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
            target_type: Some("node".to_string()),
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
            target_type: Some("person".to_string()),
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
            _source_type: &str,
            relationship: &SchemaRelationship,
        ) -> Result<Vec<String>, NodeServiceError> {
            let _ = &relationship.target_type; // Suppress unused warning
            let mut statements = Vec::new();

            // Universal Graph Architecture (Issue #783): Match production behavior
            // All nodes are stored in the 'node' table, so IN and OUT reference 'node'
            statements.push(format!(
                "DEFINE TABLE IF NOT EXISTS {} SCHEMAFULL TYPE RELATION IN node OUT node;",
                edge_table
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
                SchemaTableManager::validate_relationship_name(&relationship.name)?;
                SchemaTableManager::validate_type_name(source_type)?;
                if let Some(target_type) = &relationship.target_type {
                    SchemaTableManager::validate_type_name(target_type)?;
                }

                let edge_table = relationship.compute_edge_table_name(source_type);
                let edge_ddl =
                    self.generate_edge_table_ddl(&edge_table, source_type, relationship)?;
                statements.extend(edge_ddl);
            }

            Ok(statements)
        }
    }
}
