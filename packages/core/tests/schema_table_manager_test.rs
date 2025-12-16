//! Integration tests for SchemaTableManager
//!
//! Tests cover:
//! - DDL statement generation
//! - Type name validation
//! - Field type mapping
//! - Index generation

use anyhow::Result;
use nodespace_core::{
    db::SurrealStore,
    models::schema::{SchemaField, SchemaProtectionLevel},
    services::SchemaTableManager,
};
use std::sync::Arc;
use tempfile::TempDir;

/// Helper: Create a simple SchemaField
fn field(name: &str, field_type: &str) -> SchemaField {
    SchemaField {
        name: name.to_string(),
        field_type: field_type.to_string(),
        protection: SchemaProtectionLevel::User,
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
    }
}

/// Helper: Create an indexed SchemaField
fn indexed_field(name: &str, field_type: &str) -> SchemaField {
    SchemaField {
        name: name.to_string(),
        field_type: field_type.to_string(),
        protection: SchemaProtectionLevel::User,
        core_values: None,
        user_values: None,
        indexed: true,
        required: None,
        extensible: None,
        default: None,
        description: None,
        item_type: None,
        fields: None,
        item_fields: None,
    }
}

/// Test helper: Create a test environment
async fn create_test_env() -> Result<(Arc<SurrealStore>, SchemaTableManager, TempDir)> {
    let temp_dir = TempDir::new()?;
    let db_path = temp_dir.path().join("test.db");
    let store = Arc::new(SurrealStore::new(db_path).await?);
    let table_manager = SchemaTableManager::new(store.clone());

    Ok((store, table_manager, temp_dir))
}

// =========================================================================
// DDL Statement Generation Tests
// =========================================================================

#[tokio::test]
async fn test_generate_ddl_basic_table() -> Result<()> {
    let (_store, table_manager, _temp_dir) = create_test_env().await?;

    let fields = vec![field("title", "string")];

    let statements = table_manager.generate_ddl_statements("article", &fields)?;

    // Should have table definition, node field, and title field
    assert!(statements.len() >= 2);
    assert!(statements[0].contains("DEFINE TABLE"));
    assert!(statements[0].contains("article"));
    assert!(statements[1].contains("node"));

    Ok(())
}

#[tokio::test]
async fn test_generate_ddl_with_multiple_fields() -> Result<()> {
    let (_store, table_manager, _temp_dir) = create_test_env().await?;

    let fields = vec![
        field("title", "string"),
        indexed_field("count", "number"),
        field("active", "boolean"),
    ];

    let statements = table_manager.generate_ddl_statements("product", &fields)?;

    // Should have table def + node field + 3 fields + 1 index
    assert!(statements.len() >= 4);

    // Check for indexed field (count should have an index)
    let has_index = statements.iter().any(|s| s.contains("DEFINE INDEX"));
    assert!(has_index, "Should have index for indexed field");

    Ok(())
}

#[tokio::test]
async fn test_generate_ddl_with_indexed_field() -> Result<()> {
    let (_store, table_manager, _temp_dir) = create_test_env().await?;

    let fields = vec![indexed_field("email", "string")];

    let statements = table_manager.generate_ddl_statements("user", &fields)?;

    let index_statement = statements
        .iter()
        .find(|s| s.contains("DEFINE INDEX"))
        .expect("Should have index statement");
    assert!(index_statement.contains("email"));
    assert!(index_statement.contains("user"));

    Ok(())
}

// =========================================================================
// Type Name Validation Tests
// =========================================================================

#[tokio::test]
async fn test_generate_ddl_rejects_invalid_type_name_with_spaces() -> Result<()> {
    let (_store, table_manager, _temp_dir) = create_test_env().await?;

    let fields = vec![field("field", "string")];

    let result = table_manager.generate_ddl_statements("invalid name", &fields);
    assert!(result.is_err());

    Ok(())
}

#[tokio::test]
async fn test_generate_ddl_rejects_invalid_type_name_with_special_chars() -> Result<()> {
    let (_store, table_manager, _temp_dir) = create_test_env().await?;

    let fields = vec![field("field", "string")];

    // Test various invalid characters
    let invalid_names = vec![
        "table-name",
        "table.name",
        "table/name",
        "table;DROP TABLE",
        "table'name",
    ];

    for name in invalid_names {
        let result = table_manager.generate_ddl_statements(name, &fields);
        assert!(result.is_err(), "Should reject invalid name: {}", name);
    }

    Ok(())
}

#[tokio::test]
async fn test_generate_ddl_accepts_valid_type_names() -> Result<()> {
    let (_store, table_manager, _temp_dir) = create_test_env().await?;

    let fields = vec![field("field", "string")];

    // Test valid names
    let valid_names = vec![
        "simple",
        "with_underscore",
        "CamelCase",
        "snake_case_name",
        "MixedCase_123",
        "table123",
    ];

    for name in valid_names {
        let result = table_manager.generate_ddl_statements(name, &fields);
        assert!(result.is_ok(), "Should accept valid name: {}", name);
    }

    Ok(())
}

// =========================================================================
// Field Type Mapping Tests
// =========================================================================

#[tokio::test]
async fn test_generate_ddl_maps_string_type() -> Result<()> {
    let (_store, table_manager, _temp_dir) = create_test_env().await?;

    let fields = vec![field("name", "string")];

    let statements = table_manager.generate_ddl_statements("test", &fields)?;

    let field_stmt = statements.iter().find(|s| s.contains("name"));
    assert!(field_stmt.is_some());
    assert!(
        field_stmt.unwrap().contains("string"),
        "Should map string type"
    );

    Ok(())
}

#[tokio::test]
async fn test_generate_ddl_maps_number_type() -> Result<()> {
    let (_store, table_manager, _temp_dir) = create_test_env().await?;

    let fields = vec![field("quantity", "number")];

    let statements = table_manager.generate_ddl_statements("test", &fields)?;

    let field_stmt = statements.iter().find(|s| s.contains("quantity"));
    assert!(field_stmt.is_some());

    Ok(())
}

#[tokio::test]
async fn test_generate_ddl_maps_boolean_type() -> Result<()> {
    let (_store, table_manager, _temp_dir) = create_test_env().await?;

    let fields = vec![field("active", "boolean")];

    let statements = table_manager.generate_ddl_statements("test", &fields)?;

    let field_stmt = statements.iter().find(|s| s.contains("active"));
    assert!(field_stmt.is_some());

    Ok(())
}

#[tokio::test]
async fn test_generate_ddl_maps_date_type() -> Result<()> {
    let (_store, table_manager, _temp_dir) = create_test_env().await?;

    // The correct type is "date" (maps to SurrealDB datetime)
    let fields = vec![field("due_date", "date")];

    let statements = table_manager.generate_ddl_statements("test", &fields)?;

    let field_stmt = statements.iter().find(|s| s.contains("due_date"));
    assert!(field_stmt.is_some());

    Ok(())
}

#[tokio::test]
async fn test_generate_ddl_maps_array_type() -> Result<()> {
    let (_store, table_manager, _temp_dir) = create_test_env().await?;

    let fields = vec![field("tags", "array")];

    let statements = table_manager.generate_ddl_statements("test", &fields)?;

    let field_stmt = statements.iter().find(|s| s.contains("tags"));
    assert!(field_stmt.is_some());

    Ok(())
}

#[tokio::test]
async fn test_generate_ddl_maps_object_type() -> Result<()> {
    let (_store, table_manager, _temp_dir) = create_test_env().await?;

    let fields = vec![field("metadata", "object")];

    let statements = table_manager.generate_ddl_statements("test", &fields)?;

    let field_stmt = statements.iter().find(|s| s.contains("metadata"));
    assert!(field_stmt.is_some());

    Ok(())
}

// =========================================================================
// Nested Field Tests (using fields property)
// =========================================================================

#[tokio::test]
async fn test_generate_ddl_with_nested_fields() -> Result<()> {
    let (_store, table_manager, _temp_dir) = create_test_env().await?;

    let fields = vec![SchemaField {
        name: "address".to_string(),
        field_type: "object".to_string(),
        protection: SchemaProtectionLevel::User,
        core_values: None,
        user_values: None,
        indexed: false,
        required: None,
        extensible: None,
        default: None,
        description: None,
        item_type: None,
        fields: Some(vec![field("street", "string"), field("city", "string")]),
        item_fields: None,
    }];

    let statements = table_manager.generate_ddl_statements("contact", &fields)?;

    // Should have nested field definitions
    assert!(statements.len() >= 2);

    Ok(())
}

// =========================================================================
// Sync to Database Tests
// =========================================================================
// Note: sync_schema_to_database tests are covered in mcp_handlers_integration_test.rs
// through the handle_create_schema tests which exercise the same code paths.

// =========================================================================
// Empty Fields Tests
// =========================================================================

#[tokio::test]
async fn test_generate_ddl_with_empty_fields() -> Result<()> {
    let (_store, table_manager, _temp_dir) = create_test_env().await?;

    let fields: Vec<SchemaField> = vec![];

    let statements = table_manager.generate_ddl_statements("empty_table", &fields)?;

    // Should have at least table definition and node field
    assert!(statements.len() >= 2);
    assert!(statements[0].contains("DEFINE TABLE"));

    Ok(())
}

// =========================================================================
// Colon in Field Name Tests (Namespaced Fields)
// =========================================================================

#[tokio::test]
async fn test_generate_ddl_with_namespaced_field() -> Result<()> {
    let (_store, table_manager, _temp_dir) = create_test_env().await?;

    let fields = vec![SchemaField {
        name: "custom:field_name".to_string(),
        field_type: "string".to_string(),
        protection: SchemaProtectionLevel::User,
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
    }];

    let statements = table_manager.generate_ddl_statements("test", &fields)?;

    // Field with colon should be quoted with backticks
    let has_quoted_field = statements.iter().any(|s| s.contains("`custom:field_name`"));
    assert!(has_quoted_field, "Should quote field names with colons");

    Ok(())
}

// =========================================================================
// Multiple Index Tests
// =========================================================================

#[tokio::test]
async fn test_generate_ddl_with_multiple_indexes() -> Result<()> {
    let (_store, table_manager, _temp_dir) = create_test_env().await?;

    let fields = vec![
        indexed_field("email", "string"),
        indexed_field("username", "string"),
        field("bio", "string"),
    ];

    let statements = table_manager.generate_ddl_statements("account", &fields)?;

    // Count index definitions
    let index_count = statements
        .iter()
        .filter(|s| s.contains("DEFINE INDEX"))
        .count();
    assert_eq!(index_count, 2, "Should have 2 index definitions");

    Ok(())
}

// =========================================================================
// Protection Level Tests
// =========================================================================

#[tokio::test]
async fn test_generate_ddl_with_core_protected_field() -> Result<()> {
    let (_store, table_manager, _temp_dir) = create_test_env().await?;

    let fields = vec![SchemaField {
        name: "status".to_string(),
        field_type: "string".to_string(),
        protection: SchemaProtectionLevel::Core,
        core_values: None,
        user_values: None,
        indexed: true,
        required: Some(true),
        extensible: None,
        default: None,
        description: None,
        item_type: None,
        fields: None,
        item_fields: None,
    }];

    let statements = table_manager.generate_ddl_statements("task_schema", &fields)?;

    // Should generate DDL regardless of protection level
    assert!(statements.len() >= 3);

    Ok(())
}

// =========================================================================
// Enum Type Tests
// =========================================================================

#[tokio::test]
async fn test_generate_ddl_with_enum_field() -> Result<()> {
    let (_store, table_manager, _temp_dir) = create_test_env().await?;

    let fields = vec![SchemaField {
        name: "priority".to_string(),
        field_type: "enum".to_string(),
        protection: SchemaProtectionLevel::User,
        core_values: Some(vec![
            nodespace_core::models::schema::EnumValue {
                value: "low".to_string(),
                label: "Low".to_string(),
            },
            nodespace_core::models::schema::EnumValue {
                value: "high".to_string(),
                label: "High".to_string(),
            },
        ]),
        user_values: None,
        indexed: false,
        required: None,
        extensible: Some(true),
        default: None,
        description: None,
        item_type: None,
        fields: None,
        item_fields: None,
    }];

    let statements = table_manager.generate_ddl_statements("priority_test", &fields)?;

    // Should have field definition for priority
    let has_priority = statements.iter().any(|s| s.contains("priority"));
    assert!(has_priority, "Should have priority field definition");

    Ok(())
}

// =========================================================================
// Relationship DDL Tests
// =========================================================================

#[tokio::test]
async fn test_generate_relationship_ddl_statements() -> Result<()> {
    let (_store, table_manager, _temp_dir) = create_test_env().await?;

    let relationships = vec![nodespace_core::models::schema::SchemaRelationship {
        name: "works_at".to_string(),
        target_type: "company".to_string(),
        direction: nodespace_core::models::schema::RelationshipDirection::Out,
        cardinality: nodespace_core::models::schema::RelationshipCardinality::One,
        required: None,
        reverse_name: Some("employees".to_string()),
        reverse_cardinality: Some(nodespace_core::models::schema::RelationshipCardinality::Many),
        description: None,
        edge_table: None,
        edge_fields: None,
    }];

    let statements =
        table_manager.generate_relationship_ddl_statements("person", &relationships)?;

    // Should have edge table definition
    assert!(!statements.is_empty());
    let has_edge_table = statements
        .iter()
        .any(|s| s.contains("person_works_at_company") || s.contains("DEFINE TABLE"));
    assert!(has_edge_table, "Should define edge table");

    Ok(())
}
