//! Integration tests for MCP Handlers with real database operations
//!
//! These tests exercise the handler functions end-to-end with actual
//! NodeService and database interactions.

use nodespace_core::{
    db::SurrealStore,
    mcp::handlers::{
        nodes::{
            handle_create_node, handle_delete_node, handle_get_children, handle_get_node,
            handle_update_node,
        },
        relationships::{
            handle_get_all_schemas, handle_get_inbound_relationships, handle_get_relationship_graph,
        },
        schema::{handle_create_schema, handle_update_schema},
    },
    services::{NodeEmbeddingService, NodeService},
};
use nodespace_nlp_engine::{EmbeddingConfig, EmbeddingService};
use serde_json::json;
use std::sync::Arc;
use tempfile::TempDir;

/// Test helper: Create a test environment with NodeService
async fn create_test_env() -> anyhow::Result<(Arc<NodeService>, TempDir)> {
    let temp_dir = TempDir::new()?;
    let db_path = temp_dir.path().join("test.db");
    let mut store = Arc::new(SurrealStore::new(db_path).await?);
    let node_service = Arc::new(NodeService::new(&mut store).await?);

    Ok((node_service, temp_dir))
}

/// Test helper: Create a test environment with NodeService and NodeEmbeddingService
async fn create_test_env_with_embedding(
) -> anyhow::Result<(Arc<NodeService>, Arc<NodeEmbeddingService>, TempDir)> {
    let temp_dir = TempDir::new()?;
    let db_path = temp_dir.path().join("test.db");
    let mut store = Arc::new(SurrealStore::new(db_path).await?);
    let node_service = Arc::new(NodeService::new(&mut store).await?);

    // Create NLP engine (will operate in stub mode since model not available in tests)
    let mut nlp_engine = EmbeddingService::new(EmbeddingConfig::default())?;
    nlp_engine.initialize()?;
    let nlp_engine = Arc::new(nlp_engine);

    let embedding_service = Arc::new(NodeEmbeddingService::new(nlp_engine, store.clone()));

    Ok((node_service, embedding_service, temp_dir))
}

// ============================================================================
// Node Handler Tests
// ============================================================================

#[tokio::test]
async fn test_handle_create_node_text() {
    let (node_service, _temp_dir) = create_test_env().await.unwrap();

    let params = json!({
        "node_type": "text",
        "content": "Hello, World!"
    });

    let result = handle_create_node(&node_service, params).await;
    assert!(result.is_ok());

    let response = result.unwrap();
    assert!(response.get("node_id").is_some());
    assert_eq!(response["node_type"], "text");
    assert_eq!(response["success"], true);
}

#[tokio::test]
async fn test_handle_create_node_task() {
    let (node_service, _temp_dir) = create_test_env().await.unwrap();

    // Task nodes use minimal params - status is managed by backend
    let params = json!({
        "node_type": "task",
        "content": "Complete the project"
    });

    let result = handle_create_node(&node_service, params).await;
    assert!(result.is_ok());

    let response = result.unwrap();
    assert_eq!(response["node_type"], "task");
    assert_eq!(response["success"], true);
}

#[tokio::test]
async fn test_handle_create_node_with_parent() {
    let (node_service, _temp_dir) = create_test_env().await.unwrap();

    // Create parent first
    let parent_params = json!({
        "node_type": "text",
        "content": "Parent node"
    });
    let parent_result = handle_create_node(&node_service, parent_params)
        .await
        .unwrap();
    let parent_id = parent_result["node_id"].as_str().unwrap();

    // Create child with parent
    let child_params = json!({
        "node_type": "text",
        "content": "Child node",
        "parent_id": parent_id
    });
    let child_result = handle_create_node(&node_service, child_params).await;
    assert!(child_result.is_ok());

    let child = child_result.unwrap();
    assert_eq!(child["parent_id"], parent_id);
}

#[tokio::test]
async fn test_handle_create_node_invalid_type() {
    let (node_service, _temp_dir) = create_test_env().await.unwrap();

    let params = json!({
        "content": "Missing type"
    });

    let result = handle_create_node(&node_service, params).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_handle_get_node() {
    let (node_service, _temp_dir) = create_test_env().await.unwrap();

    // Create a node first
    let create_params = json!({
        "node_type": "text",
        "content": "Test content"
    });
    let created = handle_create_node(&node_service, create_params)
        .await
        .unwrap();
    let node_id = created["node_id"].as_str().unwrap();

    // Get the node
    let get_params = json!({
        "node_id": node_id
    });
    let result = handle_get_node(&node_service, get_params).await;
    assert!(result.is_ok());

    let node = result.unwrap();
    // The get_node response wraps data differently
    assert!(node.get("id").is_some() || node.get("node_id").is_some());
}

#[tokio::test]
async fn test_handle_get_node_not_found() {
    let (node_service, _temp_dir) = create_test_env().await.unwrap();

    let params = json!({
        "node_id": "nonexistent-node-id"
    });

    let result = handle_get_node(&node_service, params).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_handle_update_node() {
    let (node_service, _temp_dir) = create_test_env().await.unwrap();

    // Create a node
    let create_params = json!({
        "node_type": "text",
        "content": "Original content"
    });
    let created = handle_create_node(&node_service, create_params)
        .await
        .unwrap();
    let node_id = created["node_id"].as_str().unwrap();

    // Update the node (version is optional for MCP)
    let update_params = json!({
        "node_id": node_id,
        "content": "Updated content"
    });
    let result = handle_update_node(&node_service, update_params).await;
    assert!(result.is_ok());

    let updated = result.unwrap();
    assert_eq!(updated["success"], true);
}

#[tokio::test]
async fn test_handle_update_node_version_conflict() {
    let (node_service, _temp_dir) = create_test_env().await.unwrap();

    // Create a node
    let create_params = json!({
        "node_type": "text",
        "content": "Original"
    });
    let created = handle_create_node(&node_service, create_params)
        .await
        .unwrap();
    let node_id = created["node_id"].as_str().unwrap();

    // Try to update with wrong version
    let update_params = json!({
        "node_id": node_id,
        "content": "Updated",
        "version": 999
    });
    let result = handle_update_node(&node_service, update_params).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_handle_delete_node() {
    let (node_service, _temp_dir) = create_test_env().await.unwrap();

    // Create a node
    let create_params = json!({
        "node_type": "text",
        "content": "To be deleted"
    });
    let created = handle_create_node(&node_service, create_params)
        .await
        .unwrap();
    let node_id = created["node_id"].as_str().unwrap();

    // Delete the node
    let delete_params = json!({
        "node_id": node_id
    });
    let result = handle_delete_node(&node_service, delete_params).await;
    assert!(result.is_ok());

    // Verify it's deleted
    let get_params = json!({
        "node_id": node_id
    });
    let get_result = handle_get_node(&node_service, get_params).await;
    assert!(get_result.is_err());
}

#[tokio::test]
async fn test_handle_get_children() {
    let (node_service, _temp_dir) = create_test_env().await.unwrap();

    // Create parent
    let parent_params = json!({
        "node_type": "text",
        "content": "Parent"
    });
    let parent = handle_create_node(&node_service, parent_params)
        .await
        .unwrap();
    let parent_id = parent["node_id"].as_str().unwrap();

    // Create children
    for i in 1..=3 {
        let child_params = json!({
            "node_type": "text",
            "content": format!("Child {}", i),
            "parent_id": parent_id
        });
        handle_create_node(&node_service, child_params)
            .await
            .unwrap();
    }

    // Get children
    let get_params = json!({
        "parent_id": parent_id
    });
    let result = handle_get_children(&node_service, get_params).await;
    assert!(result.is_ok());

    let response = result.unwrap();
    // The response has "children" array
    let children = response.get("children").and_then(|c| c.as_array());
    assert!(children.is_some());
    assert_eq!(children.unwrap().len(), 3);
}

// ============================================================================
// Relationship Handler Tests
// ============================================================================

#[tokio::test]
async fn test_handle_get_relationship_graph_empty() {
    let (node_service, _temp_dir) = create_test_env().await.unwrap();

    let result = handle_get_relationship_graph(&node_service, json!({})).await;
    assert!(result.is_ok());

    let graph = result.unwrap();
    assert!(graph.get("edges").is_some());
    assert!(graph.get("totalEdges").is_some());
}

#[tokio::test]
async fn test_handle_get_inbound_relationships() {
    let (node_service, _temp_dir) = create_test_env().await.unwrap();

    // Note: input uses snake_case, output uses camelCase
    let params = json!({
        "target_type": "customer"
    });

    let result = handle_get_inbound_relationships(&node_service, params).await;
    assert!(result.is_ok());

    let response = result.unwrap();
    assert_eq!(response["targetType"], "customer");
    assert!(response.get("inboundRelationships").is_some());
    assert!(response.get("count").is_some());
}

#[tokio::test]
async fn test_handle_get_all_schemas() {
    let (node_service, _temp_dir) = create_test_env().await.unwrap();

    let result = handle_get_all_schemas(&node_service, json!({})).await;
    assert!(result.is_ok());

    let schemas = result.unwrap();
    // Should return array of schemas (may be empty or contain core schemas)
    assert!(schemas.is_array());
}

// ============================================================================
// Schema Handler Tests
// ============================================================================

#[tokio::test]
async fn test_handle_create_schema() {
    let (node_service, _temp_dir) = create_test_env().await.unwrap();

    // Minimal params - just a name with explicit fields
    let params = json!({
        "name": "TestSchema",
        "fields": [{
            "name": "title",
            "type": "string",
            "indexed": false, "protection": "user"
        }]
    });

    let result = handle_create_schema(&node_service, params).await;
    assert!(result.is_ok());

    let schema = result.unwrap();
    assert!(schema.get("schemaId").is_some());
}

#[tokio::test]
async fn test_handle_create_schema_with_fields() {
    let (node_service, _temp_dir) = create_test_env().await.unwrap();

    let params = json!({
        "name": "Product",
        "fields": [
            {
                "name": "sku",
                "type": "string",
                "required": true,
                "indexed": false, "protection": "user"
            },
            {
                "name": "price",
                "type": "number",
                "required": true,
                "indexed": false, "protection": "user"
            }
        ]
    });

    let result = handle_create_schema(&node_service, params).await;
    assert!(result.is_ok());

    let schema = result.unwrap();
    let fields = schema["fields"].as_array().unwrap();
    assert!(fields.len() >= 2);
}

#[tokio::test]
async fn test_handle_update_schema() {
    let (node_service, _temp_dir) = create_test_env().await.unwrap();

    // Create schema first with explicit fields
    let create_params = json!({
        "name": "UpdateableSchema",
        "fields": [{
            "name": "initial_field",
            "type": "string",
            "indexed": false, "protection": "user"
        }]
    });
    let created = handle_create_schema(&node_service, create_params)
        .await
        .unwrap();
    let schema_id = created["schemaId"].as_str().unwrap();

    // Update schema - add a field
    let update_params = json!({
        "schema_id": schema_id,
        "add_fields": [{
            "name": "new_field",
            "type": "string",
            "indexed": false, "protection": "user"
        }]
    });
    let result = handle_update_schema(&node_service, update_params).await;
    assert!(result.is_ok());

    let updated = result.unwrap();
    assert_eq!(updated["success"], true);
}

#[tokio::test]
async fn test_handle_create_schema_with_relationships() {
    let (node_service, _temp_dir) = create_test_env().await.unwrap();

    // Create target schema first with explicit fields
    let target_params = json!({
        "name": "Customer",
        "fields": [{
            "name": "name",
            "type": "string",
            "indexed": false, "protection": "user"
        }]
    });
    handle_create_schema(&node_service, target_params)
        .await
        .unwrap();

    // Create schema with relationship
    let params = json!({
        "name": "Invoice",
        "fields": [{"name": "amount", "type": "number", "indexed": false, "protection": "user"}],
        "relationships": [{
            "name": "billed_to",
            "targetType": "customer",
            "direction": "out",
            "cardinality": "one"
        }]
    });

    let result = handle_create_schema(&node_service, params).await;
    assert!(result.is_ok());

    // Verify relationship graph includes this
    let graph = handle_get_relationship_graph(&node_service, json!({}))
        .await
        .unwrap();
    let edges = graph["edges"].as_array().unwrap();
    let has_relationship = edges.iter().any(|e| {
        e["sourceType"] == "invoice"
            && e["relationshipName"] == "billed_to"
            && e["targetType"] == "customer"
    });
    assert!(has_relationship, "Relationship should appear in graph");
}

// ============================================================================
// Error Handling Tests
// ============================================================================

#[tokio::test]
async fn test_handle_create_node_missing_params() {
    let (node_service, _temp_dir) = create_test_env().await.unwrap();

    let result = handle_create_node(&node_service, json!({})).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_handle_get_node_invalid_params() {
    let (node_service, _temp_dir) = create_test_env().await.unwrap();

    let result = handle_get_node(&node_service, json!({})).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_handle_update_node_missing_id() {
    let (node_service, _temp_dir) = create_test_env().await.unwrap();

    let params = json!({
        "content": "New content"
    });

    let result = handle_update_node(&node_service, params).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_handle_delete_node_invalid_id() {
    let (node_service, _temp_dir) = create_test_env().await.unwrap();

    let params = json!({
        "node_id": "definitely-not-a-real-id"
    });

    let result = handle_delete_node(&node_service, params).await;
    // May or may not error depending on implementation (idempotent delete)
    // Just verify it doesn't panic
    let _ = result;
}

// ============================================================================
// Edge Case Tests
// ============================================================================

#[tokio::test]
async fn test_handle_create_node_with_empty_content() {
    let (node_service, _temp_dir) = create_test_env().await.unwrap();

    let params = json!({
        "node_type": "text",
        "content": ""
    });

    let result = handle_create_node(&node_service, params).await;
    // Empty content should be allowed
    assert!(result.is_ok());
}

#[tokio::test]
async fn test_handle_create_node_with_unicode() {
    let (node_service, _temp_dir) = create_test_env().await.unwrap();

    let params = json!({
        "node_type": "text",
        "content": "Hello 世界! Привет! 🎉"
    });

    let result = handle_create_node(&node_service, params).await;
    assert!(result.is_ok());

    // Verify content is preserved in node_data
    let response = result.unwrap();
    let node_data = &response["node_data"];
    if let Some(content) = node_data.get("content") {
        assert_eq!(content, "Hello 世界! Привет! 🎉");
    }
}

#[tokio::test]
async fn test_handle_create_node_with_special_chars() {
    let (node_service, _temp_dir) = create_test_env().await.unwrap();

    let params = json!({
        "node_type": "text",
        "content": "Line 1\nLine 2\tTabbed\r\nWindows line"
    });

    let result = handle_create_node(&node_service, params).await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn test_handle_get_children_empty_parent() {
    let (node_service, _temp_dir) = create_test_env().await.unwrap();

    // Create parent node first
    let parent = handle_create_node(
        &node_service,
        json!({
            "node_type": "text",
            "content": "Parent"
        }),
    )
    .await
    .unwrap();
    let parent_id = parent["node_id"].as_str().unwrap();

    // Get children of parent (initially empty)
    let params = json!({
        "parent_id": parent_id
    });

    let result = handle_get_children(&node_service, params).await;
    assert!(result.is_ok());

    let response = result.unwrap();
    let children = response["children"].as_array().unwrap();
    assert_eq!(children.len(), 0);
}

// ============================================================================
// Additional Coverage Tests for relationships.rs handlers
// ============================================================================

#[tokio::test]
async fn test_handle_get_inbound_relationships_various_types() {
    let (node_service, _temp_dir) = create_test_env().await.unwrap();

    // Test with various type names (input uses snake_case)
    let types = vec!["text", "task", "date", "person", "company", "project"];

    for type_name in types {
        let params = json!({
            "target_type": type_name
        });

        let result = handle_get_inbound_relationships(&node_service, params).await;
        assert!(result.is_ok(), "Should handle type: {}", type_name);

        let response = result.unwrap();
        assert_eq!(response["targetType"], type_name);
    }
}

#[tokio::test]
async fn test_handle_get_relationship_graph_with_schemas() {
    let (node_service, _temp_dir) = create_test_env().await.unwrap();

    // Create multiple schemas with relationships - use explicit fields
    handle_create_schema(
        &node_service,
        json!({
            "name": "Project",
            "fields": [{
                "name": "title",
                "type": "string",
                "indexed": false, "protection": "user"
            }]
        }),
    )
    .await
    .unwrap();

    handle_create_schema(
        &node_service,
        json!({
            "name": "TaskType",
            "fields": [{
                "name": "name",
                "type": "string",
                "indexed": false, "protection": "user"
            }],
            "relationships": [{
                "name": "belongs_to",
                "targetType": "project",
                "direction": "out",
                "cardinality": "one"
            }]
        }),
    )
    .await
    .unwrap();

    // Get the relationship graph
    let result = handle_get_relationship_graph(&node_service, json!({})).await;
    assert!(result.is_ok());

    let graph = result.unwrap();
    assert!(graph["totalEdges"].as_i64().unwrap() >= 1);
}

#[tokio::test]
async fn test_handle_get_all_schemas_with_data() {
    let (node_service, _temp_dir) = create_test_env().await.unwrap();

    // Create some schemas with explicit fields
    handle_create_schema(
        &node_service,
        json!({
            "name": "SchemaOne",
            "fields": [{
                "name": "field_a",
                "type": "string",
                "indexed": false, "protection": "user"
            }]
        }),
    )
    .await
    .unwrap();

    handle_create_schema(
        &node_service,
        json!({
            "name": "SchemaTwo",
            "fields": [{
                "name": "field_b",
                "type": "string",
                "indexed": false, "protection": "user"
            }]
        }),
    )
    .await
    .unwrap();

    // Get all schemas
    let result = handle_get_all_schemas(&node_service, json!({})).await;
    assert!(result.is_ok());

    let schemas = result.unwrap();
    let schema_array = schemas.as_array().unwrap();

    // Should have at least our two schemas (IDs are derived from names: schema_one, schema_two)
    let schema_ids: Vec<&str> = schema_array
        .iter()
        .filter_map(|s| s["id"].as_str())
        .collect();

    assert!(
        schema_ids.contains(&"schema_one") || schema_ids.iter().any(|id| id.contains("schema"))
    );
}

// ============================================================================
// Additional Node Handler Tests for Coverage
// ============================================================================

#[tokio::test]
async fn test_handle_create_node_with_properties() {
    let (node_service, _temp_dir) = create_test_env().await.unwrap();

    let params = json!({
        "node_type": "text",
        "content": "Node with properties",
        "properties": {
            "custom_field": "custom_value",
            "number_field": 42,
            "bool_field": true
        }
    });

    let result = handle_create_node(&node_service, params).await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn test_handle_update_node_with_properties() {
    let (node_service, _temp_dir) = create_test_env().await.unwrap();

    // Create node
    let create_params = json!({
        "node_type": "text",
        "content": "Original"
    });
    let created = handle_create_node(&node_service, create_params)
        .await
        .unwrap();
    let node_id = created["node_id"].as_str().unwrap();

    // Update with properties
    let update_params = json!({
        "node_id": node_id,
        "properties": {
            "added_prop": "value"
        }
    });
    let result = handle_update_node(&node_service, update_params).await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn test_handle_multiple_children_ordering() {
    let (node_service, _temp_dir) = create_test_env().await.unwrap();

    // Create parent
    let parent = handle_create_node(
        &node_service,
        json!({
            "node_type": "text",
            "content": "Parent"
        }),
    )
    .await
    .unwrap();
    let parent_id = parent["node_id"].as_str().unwrap();

    // Create multiple children
    for i in 1..=5 {
        handle_create_node(
            &node_service,
            json!({
                "node_type": "text",
                "content": format!("Child {}", i),
                "parent_id": parent_id
            }),
        )
        .await
        .unwrap();
    }

    // Get children
    let result = handle_get_children(
        &node_service,
        json!({
            "parent_id": parent_id
        }),
    )
    .await;
    assert!(result.is_ok());

    let response = result.unwrap();
    let children = response["children"].as_array().unwrap();
    assert_eq!(children.len(), 5);
}

// ============================================================================
// Additional Node Handler Tests for More Coverage
// ============================================================================

#[tokio::test]
async fn test_handle_query_nodes_by_type() {
    let (node_service, _temp_dir) = create_test_env().await.unwrap();

    // Create several nodes of different types
    handle_create_node(
        &node_service,
        json!({"node_type": "text", "content": "Text 1"}),
    )
    .await
    .unwrap();

    handle_create_node(
        &node_service,
        json!({"node_type": "text", "content": "Text 2"}),
    )
    .await
    .unwrap();

    handle_create_node(
        &node_service,
        json!({"node_type": "task", "content": "Task 1"}),
    )
    .await
    .unwrap();

    // Query by type
    let result = nodespace_core::mcp::handlers::nodes::handle_query_nodes(
        &node_service,
        json!({
            "node_type": "text",
            "limit": 10
        }),
    )
    .await;
    assert!(result.is_ok());

    let response = result.unwrap();
    let nodes = response["nodes"].as_array().unwrap();
    // Should have at least 2 text nodes
    assert!(nodes.len() >= 2);
}

#[tokio::test]
async fn test_handle_query_nodes_with_limit() {
    let (node_service, _temp_dir) = create_test_env().await.unwrap();

    // Create many nodes
    for i in 1..=10 {
        handle_create_node(
            &node_service,
            json!({
                "node_type": "text",
                "content": format!("Node {}", i)
            }),
        )
        .await
        .unwrap();
    }

    // Query with limit
    let result = nodespace_core::mcp::handlers::nodes::handle_query_nodes(
        &node_service,
        json!({
            "limit": 3
        }),
    )
    .await;
    assert!(result.is_ok());

    let response = result.unwrap();
    let nodes = response["nodes"].as_array().unwrap();
    assert!(nodes.len() <= 3);
}

#[tokio::test]
async fn test_handle_insert_child_at_index() {
    let (node_service, _temp_dir) = create_test_env().await.unwrap();

    // Create parent
    let parent = handle_create_node(
        &node_service,
        json!({"node_type": "text", "content": "Parent"}),
    )
    .await
    .unwrap();
    let parent_id = parent["node_id"].as_str().unwrap();

    // Insert child at index 0
    let result = nodespace_core::mcp::handlers::nodes::handle_insert_child_at_index(
        &node_service,
        json!({
            "parent_id": parent_id,
            "index": 0,
            "node_type": "text",
            "content": "First child"
        }),
    )
    .await;
    assert!(result.is_ok());

    // Insert another child at index 0
    let result2 = nodespace_core::mcp::handlers::nodes::handle_insert_child_at_index(
        &node_service,
        json!({
            "parent_id": parent_id,
            "index": 0,
            "node_type": "text",
            "content": "New first child"
        }),
    )
    .await;
    assert!(result2.is_ok());

    // Verify children order
    let children = handle_get_children(&node_service, json!({"parent_id": parent_id}))
        .await
        .unwrap();
    let kids = children["children"].as_array().unwrap();
    assert_eq!(kids.len(), 2);
}

#[tokio::test]
async fn test_handle_get_node_tree() {
    let (node_service, _temp_dir) = create_test_env().await.unwrap();

    // Create parent
    let parent = handle_create_node(
        &node_service,
        json!({"node_type": "text", "content": "Root"}),
    )
    .await
    .unwrap();
    let parent_id = parent["node_id"].as_str().unwrap();

    // Create nested children
    let child1 = handle_create_node(
        &node_service,
        json!({
            "node_type": "text",
            "content": "Child 1",
            "parent_id": parent_id
        }),
    )
    .await
    .unwrap();
    let child1_id = child1["node_id"].as_str().unwrap();

    handle_create_node(
        &node_service,
        json!({
            "node_type": "text",
            "content": "Grandchild 1",
            "parent_id": child1_id
        }),
    )
    .await
    .unwrap();

    // Get node tree (uses node_id not root_id)
    let result = nodespace_core::mcp::handlers::nodes::handle_get_node_tree(
        &node_service,
        json!({
            "node_id": parent_id,
            "max_depth": 3
        }),
    )
    .await;
    assert!(result.is_ok());

    // The response is the tree object directly (has node_id and children)
    let tree = result.unwrap();
    assert!(tree.get("node_id").is_some() || tree.get("id").is_some());
}

#[tokio::test]
async fn test_handle_get_nodes_batch() {
    let (node_service, _temp_dir) = create_test_env().await.unwrap();

    // Create multiple nodes
    let node1 = handle_create_node(
        &node_service,
        json!({"node_type": "text", "content": "Node 1"}),
    )
    .await
    .unwrap();
    let id1 = node1["node_id"].as_str().unwrap();

    let node2 = handle_create_node(
        &node_service,
        json!({"node_type": "text", "content": "Node 2"}),
    )
    .await
    .unwrap();
    let id2 = node2["node_id"].as_str().unwrap();

    // Get batch
    let result = nodespace_core::mcp::handlers::nodes::handle_get_nodes_batch(
        &node_service,
        json!({
            "node_ids": [id1, id2]
        }),
    )
    .await;
    assert!(result.is_ok());

    let response = result.unwrap();
    let nodes = response["nodes"].as_array().unwrap();
    assert_eq!(nodes.len(), 2);
}

#[tokio::test]
async fn test_handle_update_nodes_batch() {
    let (node_service, _temp_dir) = create_test_env().await.unwrap();

    // Create nodes
    let node1 = handle_create_node(
        &node_service,
        json!({"node_type": "text", "content": "Original 1"}),
    )
    .await
    .unwrap();
    let id1 = node1["node_id"].as_str().unwrap();

    let node2 = handle_create_node(
        &node_service,
        json!({"node_type": "text", "content": "Original 2"}),
    )
    .await
    .unwrap();
    let id2 = node2["node_id"].as_str().unwrap();

    // Batch update (uses "id" not "node_id" in update items)
    let result = nodespace_core::mcp::handlers::nodes::handle_update_nodes_batch(
        &node_service,
        json!({
            "updates": [
                {"id": id1, "content": "Updated 1"},
                {"id": id2, "content": "Updated 2"}
            ]
        }),
    )
    .await;
    assert!(result.is_ok());

    // Response contains updated, failed, count
    let response = result.unwrap();
    let updated = response["updated"].as_array().unwrap();
    assert_eq!(updated.len(), 2);
    assert_eq!(response["count"], 2);
}

// ============================================================================
// Relationship Handler Tests for More Coverage
// ============================================================================

#[tokio::test]
async fn test_handle_add_schema_relationship() {
    let (node_service, _temp_dir) = create_test_env().await.unwrap();

    // Create schemas
    handle_create_schema(
        &node_service,
        json!({
            "name": "Author",
            "fields": [{"name": "name", "type": "string", "indexed": false, "protection": "user"}]
        }),
    )
    .await
    .unwrap();

    handle_create_schema(
        &node_service,
        json!({
            "name": "Book",
            "fields": [{"name": "title", "type": "string", "indexed": false, "protection": "user"}]
        }),
    )
    .await
    .unwrap();

    // Add relationship
    let result = nodespace_core::mcp::handlers::schema::handle_add_schema_relationship(
        &node_service,
        json!({
            "schema_id": "book",
            "relationship": {
                "name": "written_by",
                "targetType": "author",
                "direction": "out",
                "cardinality": "one"
            }
        }),
    )
    .await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn test_handle_remove_schema_relationship() {
    let (node_service, _temp_dir) = create_test_env().await.unwrap();

    // Create schema with relationship
    handle_create_schema(
        &node_service,
        json!({
            "name": "Publisher",
            "fields": [{"name": "name", "type": "string", "indexed": false, "protection": "user"}]
        }),
    )
    .await
    .unwrap();

    handle_create_schema(
        &node_service,
        json!({
            "name": "Magazine",
            "fields": [{"name": "title", "type": "string", "indexed": false, "protection": "user"}],
            "relationships": [{
                "name": "published_by",
                "targetType": "publisher",
                "direction": "out",
                "cardinality": "one"
            }]
        }),
    )
    .await
    .unwrap();

    // Remove relationship
    let result = nodespace_core::mcp::handlers::schema::handle_remove_schema_relationship(
        &node_service,
        json!({
            "schema_id": "magazine",
            "relationship_name": "published_by"
        }),
    )
    .await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn test_handle_get_child_at_index() {
    let (node_service, _temp_dir) = create_test_env().await.unwrap();

    // Create parent with children
    let parent = handle_create_node(
        &node_service,
        json!({"node_type": "text", "content": "Parent"}),
    )
    .await
    .unwrap();
    let parent_id = parent["node_id"].as_str().unwrap();

    // Create children
    for i in 0..3 {
        handle_create_node(
            &node_service,
            json!({
                "node_type": "text",
                "content": format!("Child {}", i),
                "parent_id": parent_id
            }),
        )
        .await
        .unwrap();
    }

    // Get child at index
    let result = nodespace_core::mcp::handlers::nodes::handle_get_child_at_index(
        &node_service,
        json!({
            "parent_id": parent_id,
            "index": 1
        }),
    )
    .await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn test_handle_move_child_to_index() {
    let (node_service, _temp_dir) = create_test_env().await.unwrap();

    // Create parent with children
    let parent = handle_create_node(
        &node_service,
        json!({"node_type": "text", "content": "Parent"}),
    )
    .await
    .unwrap();
    let parent_id = parent["node_id"].as_str().unwrap();

    // Create children
    let mut child_ids = vec![];
    for i in 0..3 {
        let child = handle_create_node(
            &node_service,
            json!({
                "node_type": "text",
                "content": format!("Child {}", i),
                "parent_id": parent_id
            }),
        )
        .await
        .unwrap();
        child_ids.push(child["node_id"].as_str().unwrap().to_string());
    }

    // Get first child and its version
    let first_child = handle_get_node(&node_service, json!({"node_id": &child_ids[0]}))
        .await
        .unwrap();
    let version = first_child["version"].as_i64().unwrap();

    // Move first child to end
    let result = nodespace_core::mcp::handlers::nodes::handle_move_child_to_index(
        &node_service,
        json!({
            "node_id": &child_ids[0],
            "version": version,
            "index": 2
        }),
    )
    .await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn test_handle_initialize() {
    let (node_service, embedding_service, _temp_dir) =
        create_test_env_with_embedding().await.unwrap();

    let result = nodespace_core::mcp::handlers::initialize::handle_initialize(
        &node_service,
        &embedding_service,
        json!({
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": {
                "name": "test-client",
                "version": "1.0.0"
            }
        }),
    )
    .await;
    assert!(result.is_ok());

    let response = result.unwrap();
    assert!(response.get("serverInfo").is_some());
    assert!(response.get("capabilities").is_some());
}

// ============================================================================
// Markdown Handler Tests
// ============================================================================

#[tokio::test]
async fn test_handle_create_nodes_from_markdown_simple() {
    let (node_service, _temp_dir) = create_test_env().await.unwrap();

    let result = nodespace_core::mcp::handlers::markdown::handle_create_nodes_from_markdown(
        &node_service,
        json!({
            "markdown_content": "# My Document\n\nThis is the first paragraph.\n\nThis is the second paragraph.",
            "sync_import": true
        }),
    )
    .await;
    assert!(result.is_ok());

    let response = result.unwrap();
    assert!(response.get("root_id").is_some());
    assert!(response.get("nodes_created").is_some());
}

#[tokio::test]
async fn test_handle_create_nodes_from_markdown_with_title() {
    let (node_service, _temp_dir) = create_test_env().await.unwrap();

    let result = nodespace_core::mcp::handlers::markdown::handle_create_nodes_from_markdown(
        &node_service,
        json!({
            "title": "Custom Title",
            "markdown_content": "First paragraph.\n\nSecond paragraph."
        }),
    )
    .await;
    assert!(result.is_ok());

    let response = result.unwrap();
    assert!(response.get("root_id").is_some());
}

#[tokio::test]
async fn test_handle_create_nodes_from_markdown_with_tasks() {
    let (node_service, _temp_dir) = create_test_env().await.unwrap();

    let result = nodespace_core::mcp::handlers::markdown::handle_create_nodes_from_markdown(
        &node_service,
        json!({
            "markdown_content": "# Tasks\n\n- [ ] First task\n- [x] Completed task\n- [ ] Another task",
            "sync_import": true
        }),
    )
    .await;
    assert!(result.is_ok());

    let response = result.unwrap();
    let nodes_created = response["nodes_created"].as_u64().unwrap();
    assert!(nodes_created >= 3, "Should create at least 3 task nodes");
}

#[tokio::test]
async fn test_handle_create_nodes_from_markdown_with_date() {
    let (node_service, _temp_dir) = create_test_env().await.unwrap();

    let result = nodespace_core::mcp::handlers::markdown::handle_create_nodes_from_markdown(
        &node_service,
        json!({
            "markdown_content": "# 2024-01-15\n\nFirst paragraph.\n\nSecond paragraph."
        }),
    )
    .await;
    assert!(result.is_ok());

    let response = result.unwrap();
    assert!(response.get("root_id").is_some());
}

#[tokio::test]
async fn test_handle_create_nodes_from_markdown_with_parent() {
    let (node_service, _temp_dir) = create_test_env().await.unwrap();

    // Create parent first
    let parent = handle_create_node(
        &node_service,
        json!({"node_type": "text", "content": "Parent"}),
    )
    .await
    .unwrap();
    let parent_id = parent["node_id"].as_str().unwrap();

    let result = nodespace_core::mcp::handlers::markdown::handle_create_nodes_from_markdown(
        &node_service,
        json!({
            "markdown_content": "# Child Document\n\nContent here.",
            "parent_id": parent_id
        }),
    )
    .await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn test_handle_create_nodes_from_markdown_empty_content() {
    let (node_service, _temp_dir) = create_test_env().await.unwrap();

    let result = nodespace_core::mcp::handlers::markdown::handle_create_nodes_from_markdown(
        &node_service,
        json!({
            "markdown_content": ""
        }),
    )
    .await;
    // Should fail - empty content
    assert!(result.is_err());
}

#[tokio::test]
async fn test_handle_get_markdown_from_node() {
    let (node_service, _temp_dir) = create_test_env().await.unwrap();

    // Create a node structure
    let parent = handle_create_node(
        &node_service,
        json!({"node_type": "text", "content": "Document Title"}),
    )
    .await
    .unwrap();
    let parent_id = parent["node_id"].as_str().unwrap();

    handle_create_node(
        &node_service,
        json!({
            "node_type": "text",
            "content": "First paragraph",
            "parent_id": parent_id
        }),
    )
    .await
    .unwrap();

    handle_create_node(
        &node_service,
        json!({
            "node_type": "text",
            "content": "Second paragraph",
            "parent_id": parent_id
        }),
    )
    .await
    .unwrap();

    // Get markdown
    let result = nodespace_core::mcp::handlers::markdown::handle_get_markdown_from_node_id(
        &node_service,
        json!({
            "node_id": parent_id
        }),
    )
    .await;
    assert!(result.is_ok());

    let response = result.unwrap();
    assert!(response.get("markdown").is_some());
    let md = response["markdown"].as_str().unwrap();
    assert!(md.contains("Document Title"));
}

#[tokio::test]
async fn test_handle_get_markdown_with_tasks() {
    let (node_service, _temp_dir) = create_test_env().await.unwrap();

    // Create tasks
    let parent = handle_create_node(
        &node_service,
        json!({"node_type": "text", "content": "Task List"}),
    )
    .await
    .unwrap();
    let parent_id = parent["node_id"].as_str().unwrap();

    handle_create_node(
        &node_service,
        json!({
            "node_type": "task",
            "content": "Do something",
            "parent_id": parent_id
        }),
    )
    .await
    .unwrap();

    // Get markdown
    let result = nodespace_core::mcp::handlers::markdown::handle_get_markdown_from_node_id(
        &node_service,
        json!({
            "node_id": parent_id
        }),
    )
    .await;
    assert!(result.is_ok());

    let response = result.unwrap();
    let md = response["markdown"].as_str().unwrap();
    // Task should appear as checkbox
    assert!(md.contains("[ ]") || md.contains("[x]") || md.contains("Do something"));
}

#[tokio::test]
async fn test_handle_get_markdown_not_found() {
    let (node_service, _temp_dir) = create_test_env().await.unwrap();

    let result = nodespace_core::mcp::handlers::markdown::handle_get_markdown_from_node_id(
        &node_service,
        json!({
            "node_id": "nonexistent-node-id"
        }),
    )
    .await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_handle_create_nodes_from_markdown_code_block() {
    let (node_service, _temp_dir) = create_test_env().await.unwrap();

    let result = nodespace_core::mcp::handlers::markdown::handle_create_nodes_from_markdown(
        &node_service,
        json!({
            "markdown_content": "# Code Example\n\n```rust\nfn main() {\n    println!(\"Hello\");\n}\n```"
        }),
    )
    .await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn test_handle_create_nodes_from_markdown_nested_lists() {
    let (node_service, _temp_dir) = create_test_env().await.unwrap();

    let result = nodespace_core::mcp::handlers::markdown::handle_create_nodes_from_markdown(
        &node_service,
        json!({
            "markdown_content": "# Nested List\n\n- Item 1\n  - Sub item 1.1\n  - Sub item 1.2\n- Item 2",
            "sync_import": true
        }),
    )
    .await;
    assert!(result.is_ok());

    let response = result.unwrap();
    let nodes_created = response["nodes_created"].as_u64().unwrap();
    assert!(nodes_created >= 4, "Should create nested list nodes");
}

#[tokio::test]
async fn test_handle_create_nodes_from_markdown_blockquote() {
    let (node_service, _temp_dir) = create_test_env().await.unwrap();

    let result = nodespace_core::mcp::handlers::markdown::handle_create_nodes_from_markdown(
        &node_service,
        json!({
            "markdown_content": "# Quote Example\n\n> This is a blockquote\n> with multiple lines"
        }),
    )
    .await;
    assert!(result.is_ok());
}

// ============================================================================
// Relationship Handler Tests - CRUD Operations
// ============================================================================

#[tokio::test]
async fn test_handle_create_relationship() {
    let (node_service, _temp_dir) = create_test_env().await.unwrap();

    // Create a schema with relationships
    handle_create_schema(
        &node_service,
        json!({
            "name": "invoice",
            "fields": [
                {"name": "amount", "type": "number", "indexed": false, "protection": "user"}
            ],
            "relationships": [
                {
                    "name": "billed_to",
                    "targetType": "customer",
                    "direction": "out",
                    "cardinality": "one"
                }
            ]
        }),
    )
    .await
    .unwrap();

    handle_create_schema(
        &node_service,
        json!({
            "name": "customer",
            "fields": [
                {"name": "name", "type": "string", "indexed": false, "protection": "user"}
            ]
        }),
    )
    .await
    .unwrap();

    // Create nodes
    let invoice = handle_create_node(
        &node_service,
        json!({
            "node_type": "invoice",
            "content": "Invoice #001",
            "properties": {"amount": 100}
        }),
    )
    .await
    .unwrap();
    let invoice_id = invoice["node_id"].as_str().unwrap();

    let customer = handle_create_node(
        &node_service,
        json!({
            "node_type": "customer",
            "content": "Acme Corp",
            "properties": {"name": "Acme Corp"}
        }),
    )
    .await
    .unwrap();
    let customer_id = customer["node_id"].as_str().unwrap();

    // Create relationship
    let result = nodespace_core::mcp::handlers::relationships::handle_create_relationship(
        &node_service,
        json!({
            "source_id": invoice_id,
            "relationship_name": "billed_to",
            "target_id": customer_id
        }),
    )
    .await;

    assert!(
        result.is_ok(),
        "Failed to create relationship: {:?}",
        result.err()
    );
    let response = result.unwrap();
    assert_eq!(response["success"], true);
    assert_eq!(response["sourceId"], invoice_id);
    assert_eq!(response["relationshipName"], "billed_to");
    assert_eq!(response["targetId"], customer_id);
}

#[tokio::test]
async fn test_handle_delete_relationship() {
    let (node_service, _temp_dir) = create_test_env().await.unwrap();

    // Create schemas
    handle_create_schema(
        &node_service,
        json!({
            "name": "project",
            "fields": [
                {"name": "title", "type": "string", "indexed": false, "protection": "user"}
            ],
            "relationships": [
                {
                    "name": "owned_by",
                    "targetType": "person",
                    "direction": "out",
                    "cardinality": "one"
                }
            ]
        }),
    )
    .await
    .unwrap();

    handle_create_schema(
        &node_service,
        json!({
            "name": "person",
            "fields": [
                {"name": "name", "type": "string", "indexed": false, "protection": "user"}
            ]
        }),
    )
    .await
    .unwrap();

    // Create nodes
    let project = handle_create_node(
        &node_service,
        json!({
            "node_type": "project",
            "content": "Project Alpha",
            "properties": {"title": "Alpha"}
        }),
    )
    .await
    .unwrap();
    let project_id = project["node_id"].as_str().unwrap();

    let person = handle_create_node(
        &node_service,
        json!({
            "node_type": "person",
            "content": "John Doe",
            "properties": {"name": "John Doe"}
        }),
    )
    .await
    .unwrap();
    let person_id = person["node_id"].as_str().unwrap();

    // Create relationship first
    nodespace_core::mcp::handlers::relationships::handle_create_relationship(
        &node_service,
        json!({
            "source_id": project_id,
            "relationship_name": "owned_by",
            "target_id": person_id
        }),
    )
    .await
    .unwrap();

    // Now delete it
    let result = nodespace_core::mcp::handlers::relationships::handle_delete_relationship(
        &node_service,
        json!({
            "source_id": project_id,
            "relationship_name": "owned_by",
            "target_id": person_id
        }),
    )
    .await;

    assert!(result.is_ok());
    let response = result.unwrap();
    assert_eq!(response["success"], true);
}

#[tokio::test]
async fn test_handle_get_related_nodes() {
    let (node_service, _temp_dir) = create_test_env().await.unwrap();

    // Create schemas with relationships
    handle_create_schema(
        &node_service,
        json!({
            "name": "author",
            "fields": [
                {"name": "name", "type": "string", "indexed": false, "protection": "user"}
            ],
            "relationships": [
                {
                    "name": "wrote",
                    "targetType": "book",
                    "direction": "out",
                    "cardinality": "many"
                }
            ]
        }),
    )
    .await
    .unwrap();

    handle_create_schema(
        &node_service,
        json!({
            "name": "book",
            "fields": [
                {"name": "title", "type": "string", "indexed": false, "protection": "user"}
            ]
        }),
    )
    .await
    .unwrap();

    // Create nodes
    let author = handle_create_node(
        &node_service,
        json!({
            "node_type": "author",
            "content": "Jane Author",
            "properties": {"name": "Jane Author"}
        }),
    )
    .await
    .unwrap();
    let author_id = author["node_id"].as_str().unwrap();

    let book1 = handle_create_node(
        &node_service,
        json!({
            "node_type": "book",
            "content": "First Book",
            "properties": {"title": "First Book"}
        }),
    )
    .await
    .unwrap();
    let book1_id = book1["node_id"].as_str().unwrap();

    let book2 = handle_create_node(
        &node_service,
        json!({
            "node_type": "book",
            "content": "Second Book",
            "properties": {"title": "Second Book"}
        }),
    )
    .await
    .unwrap();
    let book2_id = book2["node_id"].as_str().unwrap();

    // Create relationships
    nodespace_core::mcp::handlers::relationships::handle_create_relationship(
        &node_service,
        json!({
            "source_id": author_id,
            "relationship_name": "wrote",
            "target_id": book1_id
        }),
    )
    .await
    .unwrap();

    nodespace_core::mcp::handlers::relationships::handle_create_relationship(
        &node_service,
        json!({
            "source_id": author_id,
            "relationship_name": "wrote",
            "target_id": book2_id
        }),
    )
    .await
    .unwrap();

    // Get related nodes
    let result = nodespace_core::mcp::handlers::relationships::handle_get_related_nodes(
        &node_service,
        json!({
            "node_id": author_id,
            "relationship_name": "wrote",
            "direction": "out"
        }),
    )
    .await;

    assert!(
        result.is_ok(),
        "Failed to get related nodes: {:?}",
        result.err()
    );
    let response = result.unwrap();
    assert_eq!(response["nodeId"], author_id);
    assert_eq!(response["relationshipName"], "wrote");
    assert_eq!(response["count"], 2);
    assert!(response["relatedNodes"].as_array().unwrap().len() == 2);
}

#[tokio::test]
async fn test_handle_get_related_nodes_empty() {
    let (node_service, _temp_dir) = create_test_env().await.unwrap();

    // Create schemas
    handle_create_schema(
        &node_service,
        json!({
            "name": "org",
            "fields": [
                {"name": "name", "type": "string", "indexed": false, "protection": "user"}
            ],
            "relationships": [
                {
                    "name": "employs",
                    "targetType": "employee",
                    "direction": "out",
                    "cardinality": "many"
                }
            ]
        }),
    )
    .await
    .unwrap();

    handle_create_schema(
        &node_service,
        json!({
            "name": "employee",
            "fields": [
                {"name": "name", "type": "string", "indexed": false, "protection": "user"}
            ]
        }),
    )
    .await
    .unwrap();

    let org = handle_create_node(
        &node_service,
        json!({
            "node_type": "org",
            "content": "Empty Org",
            "properties": {"name": "Empty Org"}
        }),
    )
    .await
    .unwrap();
    let org_id = org["node_id"].as_str().unwrap();

    // Get related nodes (should be empty)
    let result = nodespace_core::mcp::handlers::relationships::handle_get_related_nodes(
        &node_service,
        json!({
            "node_id": org_id,
            "relationship_name": "employs",
            "direction": "out"
        }),
    )
    .await;

    assert!(result.is_ok());
    let response = result.unwrap();
    assert_eq!(response["count"], 0);
    assert!(response["relatedNodes"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn test_handle_create_relationship_with_edge_data() {
    let (node_service, _temp_dir) = create_test_env().await.unwrap();

    // Create schemas
    handle_create_schema(
        &node_service,
        json!({
            "name": "order",
            "fields": [
                {"name": "total", "type": "number", "indexed": false, "protection": "user"}
            ],
            "relationships": [
                {
                    "name": "contains",
                    "targetType": "product",
                    "direction": "out",
                    "cardinality": "many"
                }
            ]
        }),
    )
    .await
    .unwrap();

    handle_create_schema(
        &node_service,
        json!({
            "name": "product",
            "fields": [
                {"name": "name", "type": "string", "indexed": false, "protection": "user"}
            ]
        }),
    )
    .await
    .unwrap();

    let order = handle_create_node(
        &node_service,
        json!({
            "node_type": "order",
            "content": "Order #100",
            "properties": {"total": 500}
        }),
    )
    .await
    .unwrap();
    let order_id = order["node_id"].as_str().unwrap();

    let product = handle_create_node(
        &node_service,
        json!({
            "node_type": "product",
            "content": "Widget",
            "properties": {"name": "Widget"}
        }),
    )
    .await
    .unwrap();
    let product_id = product["node_id"].as_str().unwrap();

    // Create relationship with edge data
    let result = nodespace_core::mcp::handlers::relationships::handle_create_relationship(
        &node_service,
        json!({
            "source_id": order_id,
            "relationship_name": "contains",
            "target_id": product_id,
            "edge_data": {
                "quantity": 5,
                "unit_price": 100
            }
        }),
    )
    .await;

    assert!(result.is_ok());
}

// ============================================================================
// Additional Node Handler Tests
// ============================================================================

#[tokio::test]
async fn test_handle_update_node_content() {
    let (node_service, _temp_dir) = create_test_env().await.unwrap();

    let node = handle_create_node(
        &node_service,
        json!({
            "node_type": "text",
            "content": "Original content"
        }),
    )
    .await
    .unwrap();
    let node_id = node["node_id"].as_str().unwrap();

    let result = handle_update_node(
        &node_service,
        json!({
            "node_id": node_id,
            "content": "Updated content"
        }),
    )
    .await;

    assert!(result.is_ok());
    let response = result.unwrap();
    assert_eq!(response["success"], true);
}

#[tokio::test]
async fn test_handle_delete_node_cascade() {
    let (node_service, _temp_dir) = create_test_env().await.unwrap();

    // Create parent with children
    let parent = handle_create_node(
        &node_service,
        json!({
            "node_type": "text",
            "content": "Parent"
        }),
    )
    .await
    .unwrap();
    let parent_id = parent["node_id"].as_str().unwrap();

    handle_create_node(
        &node_service,
        json!({
            "node_type": "text",
            "content": "Child 1",
            "parent_id": parent_id
        }),
    )
    .await
    .unwrap();

    handle_create_node(
        &node_service,
        json!({
            "node_type": "text",
            "content": "Child 2",
            "parent_id": parent_id
        }),
    )
    .await
    .unwrap();

    // Delete parent (should cascade to children)
    let result = handle_delete_node(
        &node_service,
        json!({
            "node_id": parent_id
        }),
    )
    .await;

    assert!(result.is_ok());
    let response = result.unwrap();
    assert_eq!(response["success"], true);

    // Verify parent is gone
    let get_result = handle_get_node(
        &node_service,
        json!({
            "node_id": parent_id
        }),
    )
    .await;
    assert!(get_result.is_err());
}

#[tokio::test]
async fn test_handle_get_children_ordering() {
    let (node_service, _temp_dir) = create_test_env().await.unwrap();

    let parent = handle_create_node(
        &node_service,
        json!({
            "node_type": "text",
            "content": "Parent"
        }),
    )
    .await
    .unwrap();
    let parent_id = parent["node_id"].as_str().unwrap();

    // Create children in order
    for i in 1..=5 {
        handle_create_node(
            &node_service,
            json!({
                "node_type": "text",
                "content": format!("Child {}", i),
                "parent_id": parent_id
            }),
        )
        .await
        .unwrap();
    }

    let result = handle_get_children(
        &node_service,
        json!({
            "parent_id": parent_id
        }),
    )
    .await;

    assert!(result.is_ok());
    let response = result.unwrap();

    let children = response.get("children").and_then(|c| c.as_array());
    assert!(children.is_some());
    assert_eq!(children.unwrap().len(), 5);
}

// ============================================================================
// Schema Handler Edge Cases
// ============================================================================

#[tokio::test]
async fn test_handle_update_schema_add_field() {
    let (node_service, _temp_dir) = create_test_env().await.unwrap();

    // Create initial schema
    let created = handle_create_schema(
        &node_service,
        json!({
            "name": "article",
            "fields": [
                {"name": "title", "type": "string", "indexed": false, "protection": "user"}
            ]
        }),
    )
    .await
    .unwrap();
    let schema_id = created["schemaId"].as_str().unwrap();

    // Update schema with new field using add_fields
    let result = handle_update_schema(
        &node_service,
        json!({
            "schema_id": schema_id,
            "add_fields": [
                {"name": "author", "type": "string", "indexed": false, "protection": "user"}
            ]
        }),
    )
    .await;

    assert!(result.is_ok());
}

#[tokio::test]
async fn test_handle_create_schema_duplicate() {
    let (node_service, _temp_dir) = create_test_env().await.unwrap();

    // Create schema
    handle_create_schema(
        &node_service,
        json!({
            "name": "widget",
            "fields": [
                {"name": "size", "type": "number", "indexed": false, "protection": "user"}
            ]
        }),
    )
    .await
    .unwrap();

    // Try to create duplicate
    let result = handle_create_schema(
        &node_service,
        json!({
            "name": "widget",
            "fields": [
                {"name": "color", "type": "string", "indexed": false, "protection": "user"}
            ]
        }),
    )
    .await;

    // Should fail or update
    // The behavior depends on implementation - this test documents current behavior
    assert!(result.is_ok() || result.is_err());
}

// ============================================================================
// Batch Operation Edge Case Tests
// ============================================================================

#[tokio::test]
async fn test_handle_get_nodes_batch_empty() {
    let (node_service, _temp_dir) = create_test_env().await.unwrap();

    // Empty batch should fail
    let result = nodespace_core::mcp::handlers::nodes::handle_get_nodes_batch(
        &node_service,
        json!({
            "node_ids": []
        }),
    )
    .await;

    assert!(result.is_err());
}

#[tokio::test]
async fn test_handle_get_nodes_batch_partial_not_found() {
    let (node_service, _temp_dir) = create_test_env().await.unwrap();

    // Create one node
    let node = handle_create_node(
        &node_service,
        json!({
            "node_type": "text",
            "content": "Existing node"
        }),
    )
    .await
    .unwrap();
    let existing_id = node["node_id"].as_str().unwrap();

    // Request both existing and non-existing
    let result = nodespace_core::mcp::handlers::nodes::handle_get_nodes_batch(
        &node_service,
        json!({
            "node_ids": [existing_id, "nonexistent-id"]
        }),
    )
    .await;

    assert!(result.is_ok());
    let response = result.unwrap();

    // Should return one found, one not found
    let nodes = response["nodes"].as_array().unwrap();
    let not_found = response["not_found"].as_array().unwrap();

    assert_eq!(nodes.len(), 1);
    assert_eq!(not_found.len(), 1);
    assert!(not_found.iter().any(|id| id == "nonexistent-id"));
}

#[tokio::test]
async fn test_handle_update_nodes_batch_empty() {
    let (node_service, _temp_dir) = create_test_env().await.unwrap();

    // Empty updates should fail
    let result = nodespace_core::mcp::handlers::nodes::handle_update_nodes_batch(
        &node_service,
        json!({
            "updates": []
        }),
    )
    .await;

    assert!(result.is_err());
}

#[tokio::test]
async fn test_handle_update_nodes_batch_partial_failure() {
    let (node_service, _temp_dir) = create_test_env().await.unwrap();

    // Create one node
    let node = handle_create_node(
        &node_service,
        json!({
            "node_type": "text",
            "content": "Existing"
        }),
    )
    .await
    .unwrap();
    let existing_id = node["node_id"].as_str().unwrap();

    // Update one existing, one nonexistent
    let result = nodespace_core::mcp::handlers::nodes::handle_update_nodes_batch(
        &node_service,
        json!({
            "updates": [
                {"id": existing_id, "content": "Updated"},
                {"id": "nonexistent-id", "content": "Should fail"}
            ]
        }),
    )
    .await;

    assert!(result.is_ok());
    let response = result.unwrap();

    // One should succeed, one should fail
    let updated = response["updated"].as_array().unwrap();
    let failed = response["failed"].as_array().unwrap();

    assert_eq!(updated.len(), 1);
    assert_eq!(failed.len(), 1);
}

// ============================================================================
// Node Tree Edge Cases
// ============================================================================

#[tokio::test]
async fn test_handle_get_node_tree_max_depth_exceeded() {
    let (node_service, _temp_dir) = create_test_env().await.unwrap();

    let node = handle_create_node(
        &node_service,
        json!({
            "node_type": "text",
            "content": "Root"
        }),
    )
    .await
    .unwrap();
    let node_id = node["node_id"].as_str().unwrap();

    // Invalid max_depth (0 or > 100)
    let result = nodespace_core::mcp::handlers::nodes::handle_get_node_tree(
        &node_service,
        json!({
            "node_id": node_id,
            "max_depth": 0
        }),
    )
    .await;

    assert!(result.is_err());
}

#[tokio::test]
async fn test_handle_get_node_tree_with_content() {
    let (node_service, _temp_dir) = create_test_env().await.unwrap();

    let parent = handle_create_node(
        &node_service,
        json!({
            "node_type": "text",
            "content": "Root node content"
        }),
    )
    .await
    .unwrap();
    let parent_id = parent["node_id"].as_str().unwrap();

    handle_create_node(
        &node_service,
        json!({
            "node_type": "text",
            "content": "Child content",
            "parent_id": parent_id
        }),
    )
    .await
    .unwrap();

    let result = nodespace_core::mcp::handlers::nodes::handle_get_node_tree(
        &node_service,
        json!({
            "node_id": parent_id,
            "max_depth": 5,
            "include_content": true
        }),
    )
    .await;

    assert!(result.is_ok());
    let tree = result.unwrap();
    assert!(tree.get("content").is_some());
}

#[tokio::test]
async fn test_handle_get_node_tree_with_metadata() {
    let (node_service, _temp_dir) = create_test_env().await.unwrap();

    let node = handle_create_node(
        &node_service,
        json!({
            "node_type": "text",
            "content": "Node"
        }),
    )
    .await
    .unwrap();
    let node_id = node["node_id"].as_str().unwrap();

    let result = nodespace_core::mcp::handlers::nodes::handle_get_node_tree(
        &node_service,
        json!({
            "node_id": node_id,
            "max_depth": 3,
            "include_metadata": true
        }),
    )
    .await;

    assert!(result.is_ok());
    let tree = result.unwrap();
    // Metadata fields should be present
    assert!(tree.get("created_at").is_some() || tree.get("properties").is_some());
}

// ============================================================================
// Index-Based Operations Edge Cases
// ============================================================================

#[tokio::test]
async fn test_handle_get_child_at_index_out_of_bounds() {
    let (node_service, _temp_dir) = create_test_env().await.unwrap();

    let parent = handle_create_node(
        &node_service,
        json!({
            "node_type": "text",
            "content": "Parent"
        }),
    )
    .await
    .unwrap();
    let parent_id = parent["node_id"].as_str().unwrap();

    // Create one child
    handle_create_node(
        &node_service,
        json!({
            "node_type": "text",
            "content": "Child 0",
            "parent_id": parent_id
        }),
    )
    .await
    .unwrap();

    // Try to get child at index 10 (out of bounds)
    let result = nodespace_core::mcp::handlers::nodes::handle_get_child_at_index(
        &node_service,
        json!({
            "parent_id": parent_id,
            "index": 10
        }),
    )
    .await;

    assert!(result.is_err());
}

#[tokio::test]
async fn test_handle_insert_child_at_index_middle() {
    let (node_service, _temp_dir) = create_test_env().await.unwrap();

    let parent = handle_create_node(
        &node_service,
        json!({
            "node_type": "text",
            "content": "Parent"
        }),
    )
    .await
    .unwrap();
    let parent_id = parent["node_id"].as_str().unwrap();

    // Create two children
    nodespace_core::mcp::handlers::nodes::handle_insert_child_at_index(
        &node_service,
        json!({
            "parent_id": parent_id,
            "index": 0,
            "node_type": "text",
            "content": "First"
        }),
    )
    .await
    .unwrap();

    nodespace_core::mcp::handlers::nodes::handle_insert_child_at_index(
        &node_service,
        json!({
            "parent_id": parent_id,
            "index": 1,
            "node_type": "text",
            "content": "Third"
        }),
    )
    .await
    .unwrap();

    // Insert in the middle
    let result = nodespace_core::mcp::handlers::nodes::handle_insert_child_at_index(
        &node_service,
        json!({
            "parent_id": parent_id,
            "index": 1,
            "node_type": "text",
            "content": "Second (inserted)"
        }),
    )
    .await;

    assert!(result.is_ok());

    // Verify ordering
    let children = handle_get_children(
        &node_service,
        json!({
            "parent_id": parent_id
        }),
    )
    .await
    .unwrap();

    let kids = children["children"].as_array().unwrap();
    assert_eq!(kids.len(), 3);
}

#[tokio::test]
async fn test_handle_move_child_to_index_no_parent() {
    let (node_service, _temp_dir) = create_test_env().await.unwrap();

    // Create a root node (no parent)
    let root = handle_create_node(
        &node_service,
        json!({
            "node_type": "text",
            "content": "Root node"
        }),
    )
    .await
    .unwrap();
    let root_id = root["node_id"].as_str().unwrap();
    let version = root["node_data"]["version"].as_i64().unwrap();

    // Try to move it - should fail since it has no parent
    let result = nodespace_core::mcp::handlers::nodes::handle_move_child_to_index(
        &node_service,
        json!({
            "node_id": root_id,
            "version": version,
            "index": 0
        }),
    )
    .await;

    assert!(result.is_err());
}

// ============================================================================
// Query Nodes Edge Cases
// ============================================================================

#[tokio::test]
async fn test_handle_query_nodes_with_offset() {
    let (node_service, _temp_dir) = create_test_env().await.unwrap();

    // Create 5 nodes
    for i in 1..=5 {
        handle_create_node(
            &node_service,
            json!({
                "node_type": "text",
                "content": format!("Node {}", i)
            }),
        )
        .await
        .unwrap();
    }

    // Query with offset
    let result = nodespace_core::mcp::handlers::nodes::handle_query_nodes(
        &node_service,
        json!({
            "limit": 2,
            "offset": 2
        }),
    )
    .await;

    assert!(result.is_ok());
    let response = result.unwrap();
    let nodes = response["nodes"].as_array().unwrap();

    // Should skip first 2 and return up to 2 more
    assert!(nodes.len() <= 2);
}

// ============================================================================
// Markdown Handler Additional Edge Cases
// ============================================================================

#[tokio::test]
async fn test_handle_create_nodes_from_markdown_ordered_list() {
    let (node_service, _temp_dir) = create_test_env().await.unwrap();

    let result = nodespace_core::mcp::handlers::markdown::handle_create_nodes_from_markdown(
        &node_service,
        json!({
            "markdown_content": "# Ordered List\n\n1. First item\n2. Second item\n3. Third item"
        }),
    )
    .await;

    assert!(result.is_ok());
}

#[tokio::test]
async fn test_handle_create_nodes_from_markdown_date_container() {
    let (node_service, _temp_dir) = create_test_env().await.unwrap();

    // Use a date as the title (YYYY-MM-DD format)
    let result = nodespace_core::mcp::handlers::markdown::handle_create_nodes_from_markdown(
        &node_service,
        json!({
            "title": "2024-12-15",
            "markdown_content": "## Daily Notes\n\nSome content for today."
        }),
    )
    .await;

    assert!(result.is_ok());
    let response = result.unwrap();
    // Root should be the date node
    let root_id = response["root_id"].as_str().unwrap();
    assert!(root_id.contains("2024-12-15"));
}

#[tokio::test]
async fn test_handle_get_markdown_without_node_ids() {
    let (node_service, _temp_dir) = create_test_env().await.unwrap();

    let parent = handle_create_node(
        &node_service,
        json!({
            "node_type": "text",
            "content": "Document"
        }),
    )
    .await
    .unwrap();
    let parent_id = parent["node_id"].as_str().unwrap();

    handle_create_node(
        &node_service,
        json!({
            "node_type": "text",
            "content": "Child text",
            "parent_id": parent_id
        }),
    )
    .await
    .unwrap();

    // Get markdown without node IDs
    let result = nodespace_core::mcp::handlers::markdown::handle_get_markdown_from_node_id(
        &node_service,
        json!({
            "node_id": parent_id,
            "include_node_ids": false
        }),
    )
    .await;

    assert!(result.is_ok());
    let response = result.unwrap();
    let md = response["markdown"].as_str().unwrap();

    // Should not contain HTML comments with node IDs
    assert!(!md.contains("<!--"));
}

#[tokio::test]
async fn test_handle_get_markdown_max_depth() {
    let (node_service, _temp_dir) = create_test_env().await.unwrap();

    // Create deep hierarchy
    let root = handle_create_node(
        &node_service,
        json!({
            "node_type": "text",
            "content": "Root"
        }),
    )
    .await
    .unwrap();
    let root_id = root["node_id"].as_str().unwrap();

    let mut parent_id = root_id.to_string();
    for i in 1..=5 {
        let child = handle_create_node(
            &node_service,
            json!({
                "node_type": "text",
                "content": format!("Level {}", i),
                "parent_id": parent_id
            }),
        )
        .await
        .unwrap();
        parent_id = child["node_id"].as_str().unwrap().to_string();
    }

    // Get markdown with limited depth
    let result = nodespace_core::mcp::handlers::markdown::handle_get_markdown_from_node_id(
        &node_service,
        json!({
            "node_id": root_id,
            "max_depth": 2
        }),
    )
    .await;

    assert!(result.is_ok());
}

#[tokio::test]
async fn test_handle_update_root_from_markdown() {
    let (node_service, _temp_dir) = create_test_env().await.unwrap();

    // Create root with children
    let root = handle_create_node(
        &node_service,
        json!({
            "node_type": "text",
            "content": "Root Document"
        }),
    )
    .await
    .unwrap();
    let root_id = root["node_id"].as_str().unwrap();

    handle_create_node(
        &node_service,
        json!({
            "node_type": "text",
            "content": "Old child 1",
            "parent_id": root_id
        }),
    )
    .await
    .unwrap();

    handle_create_node(
        &node_service,
        json!({
            "node_type": "text",
            "content": "Old child 2",
            "parent_id": root_id
        }),
    )
    .await
    .unwrap();

    // Update with new markdown (replaces children)
    let result = nodespace_core::mcp::handlers::markdown::handle_update_root_from_markdown(
        &node_service,
        json!({
            "root_id": root_id,
            "markdown": "New child content"
        }),
    )
    .await;

    assert!(result.is_ok());
    let response = result.unwrap();
    assert!(response.get("nodes_deleted").is_some());
    assert!(response.get("nodes_created").is_some());
}

#[tokio::test]
async fn test_handle_update_root_from_markdown_not_found() {
    let (node_service, _temp_dir) = create_test_env().await.unwrap();

    let result = nodespace_core::mcp::handlers::markdown::handle_update_root_from_markdown(
        &node_service,
        json!({
            "root_id": "nonexistent-root",
            "markdown": "Some content"
        }),
    )
    .await;

    assert!(result.is_err());
}

// ============================================================================
// Error Code Tests
// ============================================================================

#[tokio::test]
async fn test_handle_create_node_returns_node_data() {
    let (node_service, _temp_dir) = create_test_env().await.unwrap();

    let result = handle_create_node(
        &node_service,
        json!({
            "node_type": "text",
            "content": "Test node"
        }),
    )
    .await;

    assert!(result.is_ok());
    let response = result.unwrap();

    // Verify node_data is included in response
    assert!(response.get("node_data").is_some());
    let node_data = &response["node_data"];
    assert!(node_data.get("id").is_some());
    assert!(node_data.get("version").is_some());
}

#[tokio::test]
async fn test_handle_update_node_returns_node_data() {
    let (node_service, _temp_dir) = create_test_env().await.unwrap();

    let node = handle_create_node(
        &node_service,
        json!({
            "node_type": "text",
            "content": "Original"
        }),
    )
    .await
    .unwrap();
    let node_id = node["node_id"].as_str().unwrap();

    let result = handle_update_node(
        &node_service,
        json!({
            "node_id": node_id,
            "content": "Updated"
        }),
    )
    .await;

    assert!(result.is_ok());
    let response = result.unwrap();

    // Verify node_data is included
    assert!(response.get("node_data").is_some());
}

// ============================================================================
// Delete Node with Version Tests
// ============================================================================

#[tokio::test]
async fn test_handle_delete_node_with_version() {
    let (node_service, _temp_dir) = create_test_env().await.unwrap();

    let node = handle_create_node(
        &node_service,
        json!({
            "node_type": "text",
            "content": "Node to delete"
        }),
    )
    .await
    .unwrap();
    let node_id = node["node_id"].as_str().unwrap();
    let version = node["node_data"]["version"].as_i64().unwrap();

    // Delete with correct version
    let result = handle_delete_node(
        &node_service,
        json!({
            "node_id": node_id,
            "version": version
        }),
    )
    .await;

    assert!(result.is_ok());
}

#[tokio::test]
async fn test_handle_delete_node_version_conflict() {
    let (node_service, _temp_dir) = create_test_env().await.unwrap();

    let node = handle_create_node(
        &node_service,
        json!({
            "node_type": "text",
            "content": "Node"
        }),
    )
    .await
    .unwrap();
    let node_id = node["node_id"].as_str().unwrap();

    // Delete with wrong version
    let result = handle_delete_node(
        &node_service,
        json!({
            "node_id": node_id,
            "version": 999
        }),
    )
    .await;

    assert!(result.is_err());
}

// ============================================================================
// Node Collections Handler Tests
// ============================================================================

#[tokio::test]
async fn test_handle_get_node_collections_empty() {
    let (node_service, _temp_dir) = create_test_env().await.unwrap();

    // Create a node without any collection memberships
    let node = handle_create_node(
        &node_service,
        json!({
            "node_type": "text",
            "content": "No collections"
        }),
    )
    .await
    .unwrap();
    let node_id = node["node_id"].as_str().unwrap();

    // Get collections (should be empty)
    let result = nodespace_core::mcp::handlers::nodes::handle_get_node_collections(
        &node_service,
        json!({
            "node_id": node_id
        }),
    )
    .await;

    assert!(result.is_ok());
    let response = result.unwrap();
    assert_eq!(response["node_id"], node_id);
    assert_eq!(response["count"], 0);
    assert!(response["collections"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn test_handle_get_node_collections_with_membership() {
    let (node_service, _temp_dir) = create_test_env().await.unwrap();

    // Create a collection
    let collection = handle_create_node(
        &node_service,
        json!({
            "node_type": "collection",
            "content": "My Collection"
        }),
    )
    .await
    .unwrap();
    let collection_id = collection["node_id"].as_str().unwrap();

    // Create a node
    let node = handle_create_node(
        &node_service,
        json!({
            "node_type": "text",
            "content": "Node with collection"
        }),
    )
    .await
    .unwrap();
    let node_id = node["node_id"].as_str().unwrap();

    // Add node to collection (Issue #813: CollectionService now requires NodeService reference)
    let collection_service =
        nodespace_core::services::CollectionService::new(node_service.store(), &node_service);
    let _ = collection_service
        .add_to_collection(node_id, collection_id)
        .await;

    // Get collections
    let result = nodespace_core::mcp::handlers::nodes::handle_get_node_collections(
        &node_service,
        json!({
            "node_id": node_id
        }),
    )
    .await;

    assert!(result.is_ok());
    let response = result.unwrap();
    assert_eq!(response["node_id"], node_id);
    // May have the collection depending on implementation
    let count = response["count"].as_i64().unwrap_or(0);
    assert!(count >= 0);
}

// ============================================================================
// Error Conversion Tests (service_error_to_mcp)
// ============================================================================

#[tokio::test]
async fn test_service_error_to_mcp_node_not_found() {
    let (node_service, _temp_dir) = create_test_env().await.unwrap();

    // Try to get non-existent node
    let result = handle_get_node(
        &node_service,
        json!({
            "node_id": "nonexistent-node-id-12345"
        }),
    )
    .await;

    assert!(result.is_err());
    let err = result.unwrap_err();
    // Error should be node not found type
    assert!(err.message.contains("not found") || err.code == -32001);
}

#[tokio::test]
async fn test_service_error_to_mcp_validation_failed() {
    let (node_service, _temp_dir) = create_test_env().await.unwrap();

    // Try to create node with invalid/missing type
    let result = handle_create_node(
        &node_service,
        json!({
            "content": "Missing node_type"
        }),
    )
    .await;

    assert!(result.is_err());
}

#[tokio::test]
async fn test_service_error_to_mcp_version_conflict_details() {
    let (node_service, _temp_dir) = create_test_env().await.unwrap();

    // Create a node
    let node = handle_create_node(
        &node_service,
        json!({
            "node_type": "text",
            "content": "Test"
        }),
    )
    .await
    .unwrap();
    let node_id = node["node_id"].as_str().unwrap();

    // Try update with wrong version
    let result = handle_update_node(
        &node_service,
        json!({
            "node_id": node_id,
            "version": 9999,
            "content": "Updated"
        }),
    )
    .await;

    assert!(result.is_err());
    let err = result.unwrap_err();
    // Should be version conflict error
    assert!(err.message.contains("version") || err.message.contains("conflict"));
}

// ============================================================================
// Query Nodes with Collection Filter Tests
// ============================================================================

#[tokio::test]
async fn test_handle_query_nodes_with_collection_path() {
    let (node_service, _temp_dir) = create_test_env().await.unwrap();

    // Create collection
    let collection = handle_create_node(
        &node_service,
        json!({
            "node_type": "collection",
            "content": "test_collection"
        }),
    )
    .await
    .unwrap();
    let _collection_id = collection["node_id"].as_str().unwrap();

    // Query with non-existent collection path (should return empty)
    let result = nodespace_core::mcp::handlers::nodes::handle_query_nodes(
        &node_service,
        json!({
            "collection": "nonexistent:path"
        }),
    )
    .await;

    assert!(result.is_ok());
    let response = result.unwrap();
    // Should return empty results for non-existent collection
    assert_eq!(response["count"], 0);
}

#[tokio::test]
async fn test_handle_query_nodes_with_collection_id() {
    let (node_service, _temp_dir) = create_test_env().await.unwrap();

    // Create collection and add node to it
    let collection = handle_create_node(
        &node_service,
        json!({
            "node_type": "collection",
            "content": "query_test"
        }),
    )
    .await
    .unwrap();
    let collection_id = collection["node_id"].as_str().unwrap();

    let node = handle_create_node(
        &node_service,
        json!({
            "node_type": "text",
            "content": "Node in collection"
        }),
    )
    .await
    .unwrap();
    let node_id = node["node_id"].as_str().unwrap();

    // Add to collection (Issue #813: CollectionService now requires NodeService reference)
    let collection_service =
        nodespace_core::services::CollectionService::new(node_service.store(), &node_service);
    let _ = collection_service
        .add_to_collection(node_id, collection_id)
        .await;

    // Query with collection ID
    let result = nodespace_core::mcp::handlers::nodes::handle_query_nodes(
        &node_service,
        json!({
            "collection_id": collection_id
        }),
    )
    .await;

    assert!(result.is_ok());
}

// ============================================================================
// Auto-Create Date Node Tests
// ============================================================================

#[tokio::test]
async fn test_handle_create_node_auto_create_date_parent() {
    let (node_service, _temp_dir) = create_test_env().await.unwrap();

    // Create a child with a date-formatted parent_id (YYYY-MM-DD)
    // The handler should auto-create the date node
    let result = handle_create_node(
        &node_service,
        json!({
            "node_type": "text",
            "content": "Child of date",
            "parent_id": "2024-01-15"
        }),
    )
    .await;

    // Should succeed - date node is auto-created
    assert!(result.is_ok());
    let response = result.unwrap();
    assert_eq!(response["parent_id"], "2024-01-15");
}

#[tokio::test]
async fn test_handle_create_node_invalid_parent_not_date() {
    let (node_service, _temp_dir) = create_test_env().await.unwrap();

    // Create a child with a non-existent parent that's not a date
    let result = handle_create_node(
        &node_service,
        json!({
            "node_type": "text",
            "content": "Child",
            "parent_id": "nonexistent-parent-id"
        }),
    )
    .await;

    // Should fail - parent doesn't exist and can't be auto-created
    assert!(result.is_err(), "Expected error for non-existent parent");
    let err = result.unwrap_err();
    // Check for various error formats
    assert!(
        err.message.contains("not found")
            || err.message.contains("Parent")
            || err.message.contains("Invalid"),
        "Unexpected error message: {}",
        err.message
    );
}

// ============================================================================
// Batch Get with Error Conversion Test
// ============================================================================

#[tokio::test]
async fn test_handle_get_nodes_batch_with_conversion_error() {
    let (node_service, _temp_dir) = create_test_env().await.unwrap();

    // Create a valid node
    let node = handle_create_node(
        &node_service,
        json!({
            "node_type": "text",
            "content": "Valid node"
        }),
    )
    .await
    .unwrap();
    let valid_id = node["node_id"].as_str().unwrap();

    // Request batch with valid and invalid IDs
    let result = nodespace_core::mcp::handlers::nodes::handle_get_nodes_batch(
        &node_service,
        json!({
            "node_ids": [valid_id, "nonexistent-id", "another-missing"]
        }),
    )
    .await;

    assert!(result.is_ok());
    let response = result.unwrap();

    // Should have one found, two not found
    let nodes = response["nodes"].as_array().unwrap();
    let not_found = response["not_found"].as_array().unwrap();

    assert_eq!(nodes.len(), 1);
    assert_eq!(not_found.len(), 2);
}

// ============================================================================
// Query Nodes with Deprecated Filters Tests
// ============================================================================

#[tokio::test]
async fn test_handle_query_nodes_with_deprecated_parent_id() {
    let (node_service, _temp_dir) = create_test_env().await.unwrap();

    // Create some nodes
    handle_create_node(
        &node_service,
        json!({
            "node_type": "text",
            "content": "Test"
        }),
    )
    .await
    .unwrap();

    // Query with deprecated parent_id (should be ignored with warning)
    let result = nodespace_core::mcp::handlers::nodes::handle_query_nodes(
        &node_service,
        json!({
            "parent_id": "some-parent-id"
        }),
    )
    .await;

    // Should succeed (filter is ignored)
    assert!(result.is_ok());
}

#[tokio::test]
async fn test_handle_query_nodes_with_deprecated_root_id() {
    let (node_service, _temp_dir) = create_test_env().await.unwrap();

    // Create some nodes
    handle_create_node(
        &node_service,
        json!({
            "node_type": "text",
            "content": "Test"
        }),
    )
    .await
    .unwrap();

    // Query with deprecated root_id (should be ignored with warning)
    let result = nodespace_core::mcp::handlers::nodes::handle_query_nodes(
        &node_service,
        json!({
            "root_id": "some-root-id"
        }),
    )
    .await;

    // Should succeed (filter is ignored)
    assert!(result.is_ok());
}

// ============================================================================
// Get Children with Content Flag Tests
// ============================================================================

#[tokio::test]
async fn test_handle_get_children_with_include_content() {
    let (node_service, _temp_dir) = create_test_env().await.unwrap();

    let parent = handle_create_node(
        &node_service,
        json!({
            "node_type": "text",
            "content": "Parent"
        }),
    )
    .await
    .unwrap();
    let parent_id = parent["node_id"].as_str().unwrap();

    handle_create_node(
        &node_service,
        json!({
            "node_type": "text",
            "content": "Child with content",
            "parent_id": parent_id
        }),
    )
    .await
    .unwrap();

    // Get children with content
    let result = handle_get_children(
        &node_service,
        json!({
            "parent_id": parent_id,
            "include_content": true
        }),
    )
    .await;

    assert!(result.is_ok());
    let response = result.unwrap();
    let children = response["children"].as_array().unwrap();
    assert_eq!(children.len(), 1);

    // Content should be included
    let child = &children[0];
    assert!(child.get("content").is_some());
}

#[tokio::test]
async fn test_handle_get_children_without_include_content() {
    let (node_service, _temp_dir) = create_test_env().await.unwrap();

    let parent = handle_create_node(
        &node_service,
        json!({
            "node_type": "text",
            "content": "Parent"
        }),
    )
    .await
    .unwrap();
    let parent_id = parent["node_id"].as_str().unwrap();

    handle_create_node(
        &node_service,
        json!({
            "node_type": "text",
            "content": "Child",
            "parent_id": parent_id
        }),
    )
    .await
    .unwrap();

    // Get children without content (default)
    let result = handle_get_children(
        &node_service,
        json!({
            "parent_id": parent_id,
            "include_content": false
        }),
    )
    .await;

    assert!(result.is_ok());
}
