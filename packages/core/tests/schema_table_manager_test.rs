//! Integration tests for SchemaTableManager
//!
//! Universal Graph Architecture (Issue #783): SchemaTableManager only generates
//! DDL for relationship edge tables. Spoke tables are no longer used - all node
//! properties are stored in node.properties.
//!
//! Tests cover:
//! - Relationship edge table DDL generation
//! - Validation of relationship and type names

use anyhow::Result;
use nodespace_core::{
    db::SurrealStore,
    models::schema::{
        EdgeField, RelationshipCardinality, RelationshipDirection, SchemaRelationship,
    },
    services::SchemaTableManager,
};
use std::sync::Arc;
use tempfile::TempDir;

/// Test helper: Create a test environment
async fn create_test_env() -> Result<(Arc<SurrealStore>, SchemaTableManager, TempDir)> {
    let temp_dir = TempDir::new()?;
    let db_path = temp_dir.path().join("test.db");
    let store = Arc::new(SurrealStore::new(db_path).await?);
    let table_manager = SchemaTableManager::new();

    Ok((store, table_manager, temp_dir))
}

// =========================================================================
// Relationship DDL Tests
// =========================================================================

#[tokio::test]
async fn test_generate_relationship_ddl_statements() -> Result<()> {
    let (_store, table_manager, _temp_dir) = create_test_env().await?;

    let relationships = vec![SchemaRelationship {
        name: "works_at".to_string(),
        target_type: Some("company".to_string()),
        direction: RelationshipDirection::Out,
        cardinality: RelationshipCardinality::One,
        required: None,
        reverse_name: Some("employees".to_string()),
        reverse_cardinality: Some(RelationshipCardinality::Many),
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

#[tokio::test]
async fn test_generate_relationship_ddl_with_edge_fields() -> Result<()> {
    let (_store, table_manager, _temp_dir) = create_test_env().await?;

    let relationships = vec![SchemaRelationship {
        name: "assigned_to".to_string(),
        target_type: Some("user".to_string()),
        direction: RelationshipDirection::Out,
        cardinality: RelationshipCardinality::Many,
        required: None,
        reverse_name: None,
        reverse_cardinality: None,
        description: None,
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
    }];

    let statements = table_manager.generate_relationship_ddl_statements("task", &relationships)?;

    // Should have edge field definitions
    let has_role_field = statements.iter().any(|s| s.contains("role"));
    assert!(has_role_field, "Should have role field definition");

    let has_assigned_at_field = statements.iter().any(|s| s.contains("assigned_at"));
    assert!(
        has_assigned_at_field,
        "Should have assigned_at field definition"
    );

    // Should have index for role (marked as indexed)
    let has_role_index = statements
        .iter()
        .any(|s| s.contains("INDEX") && s.contains("role"));
    assert!(has_role_index, "Should have index for role field");

    Ok(())
}

#[tokio::test]
async fn test_generate_relationship_ddl_multiple_relationships() -> Result<()> {
    let (_store, table_manager, _temp_dir) = create_test_env().await?;

    let relationships = vec![
        SchemaRelationship {
            name: "billed_to".to_string(),
            target_type: Some("customer".to_string()),
            direction: RelationshipDirection::Out,
            cardinality: RelationshipCardinality::One,
            required: None,
            reverse_name: None,
            reverse_cardinality: None,
            description: None,
            edge_table: None,
            edge_fields: None,
        },
        SchemaRelationship {
            name: "shipped_to".to_string(),
            target_type: Some("address".to_string()),
            direction: RelationshipDirection::Out,
            cardinality: RelationshipCardinality::One,
            required: None,
            reverse_name: None,
            reverse_cardinality: None,
            description: None,
            edge_table: None,
            edge_fields: None,
        },
    ];

    let statements =
        table_manager.generate_relationship_ddl_statements("invoice", &relationships)?;

    // Should have DDL for both relationships
    let has_billed_to = statements
        .iter()
        .any(|s| s.contains("invoice_billed_to_customer"));
    let has_shipped_to = statements
        .iter()
        .any(|s| s.contains("invoice_shipped_to_address"));

    assert!(has_billed_to, "Should have billed_to edge table");
    assert!(has_shipped_to, "Should have shipped_to edge table");

    Ok(())
}

#[tokio::test]
async fn test_generate_relationship_ddl_custom_edge_table() -> Result<()> {
    let (_store, table_manager, _temp_dir) = create_test_env().await?;

    let relationships = vec![SchemaRelationship {
        name: "collaborates_with".to_string(),
        target_type: Some("person".to_string()),
        direction: RelationshipDirection::Out,
        cardinality: RelationshipCardinality::Many,
        required: None,
        reverse_name: None,
        reverse_cardinality: None,
        description: None,
        edge_table: Some("collaborations".to_string()),
        edge_fields: None,
    }];

    let statements =
        table_manager.generate_relationship_ddl_statements("project", &relationships)?;

    // Should use custom edge table name
    let has_custom_table = statements
        .iter()
        .any(|s| s.contains("DEFINE TABLE IF NOT EXISTS collaborations"));
    assert!(has_custom_table, "Should use custom edge table name");

    // Should NOT use auto-generated name
    let has_auto_name = statements
        .iter()
        .any(|s| s.contains("project_collaborates_with_person"));
    assert!(
        !has_auto_name,
        "Should NOT have auto-generated edge table name"
    );

    Ok(())
}

#[tokio::test]
async fn test_generate_relationship_ddl_rejects_reserved_names() -> Result<()> {
    let (_store, table_manager, _temp_dir) = create_test_env().await?;

    let relationships = vec![SchemaRelationship {
        name: "has_child".to_string(), // Reserved!
        target_type: Some("node".to_string()),
        direction: RelationshipDirection::Out,
        cardinality: RelationshipCardinality::Many,
        required: None,
        reverse_name: None,
        reverse_cardinality: None,
        description: None,
        edge_table: None,
        edge_fields: None,
    }];

    let result = table_manager.generate_relationship_ddl_statements("parent", &relationships);
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("reserved"));

    Ok(())
}

#[tokio::test]
async fn test_generate_relationship_ddl_edge_table_references_node() -> Result<()> {
    let (_store, table_manager, _temp_dir) = create_test_env().await?;

    let relationships = vec![SchemaRelationship {
        name: "owns".to_string(),
        target_type: Some("asset".to_string()),
        direction: RelationshipDirection::Out,
        cardinality: RelationshipCardinality::Many,
        required: None,
        reverse_name: None,
        reverse_cardinality: None,
        description: None,
        edge_table: None,
        edge_fields: None,
    }];

    let statements = table_manager.generate_relationship_ddl_statements("owner", &relationships)?;

    // Universal Graph Architecture: Edge tables reference 'node' table
    let table_def = statements
        .iter()
        .find(|s| s.contains("DEFINE TABLE") && s.contains("RELATION"))
        .expect("Should have table definition");

    assert!(
        table_def.contains("IN node OUT node"),
        "Edge table should reference 'node' table for IN and OUT"
    );

    Ok(())
}

#[tokio::test]
async fn test_generate_relationship_ddl_includes_core_indexes() -> Result<()> {
    let (_store, table_manager, _temp_dir) = create_test_env().await?;

    let relationships = vec![SchemaRelationship {
        name: "follows".to_string(),
        target_type: Some("user".to_string()),
        direction: RelationshipDirection::Out,
        cardinality: RelationshipCardinality::Many,
        required: None,
        reverse_name: None,
        reverse_cardinality: None,
        description: None,
        edge_table: None,
        edge_fields: None,
    }];

    let statements = table_manager.generate_relationship_ddl_statements("user", &relationships)?;

    // Should have index on 'in' for forward traversal
    let has_in_index = statements
        .iter()
        .any(|s| s.contains("INDEX") && s.contains("_in"));
    assert!(has_in_index, "Should have index on 'in' column");

    // Should have index on 'out' for reverse traversal
    let has_out_index = statements
        .iter()
        .any(|s| s.contains("INDEX") && s.contains("_out"));
    assert!(has_out_index, "Should have index on 'out' column");

    // Should have unique index on (in, out)
    let has_unique_index = statements.iter().any(|s| s.contains("UNIQUE"));
    assert!(has_unique_index, "Should have unique index on (in, out)");

    Ok(())
}

#[tokio::test]
async fn test_generate_relationship_ddl_empty_relationships() -> Result<()> {
    let (_store, table_manager, _temp_dir) = create_test_env().await?;

    let relationships: Vec<SchemaRelationship> = vec![];

    let statements =
        table_manager.generate_relationship_ddl_statements("empty_schema", &relationships)?;

    // Should return empty vector for no relationships
    assert!(
        statements.is_empty(),
        "Should return empty statements for no relationships"
    );

    Ok(())
}
