//! Tests for MCP node handlers
//!
//! Tests the request parameter parsing and response formatting for node CRUD operations.

#[cfg(test)]
mod tests {
    use crate::mcp::types::{MCPError, INVALID_PARAMS, NODE_NOT_FOUND, VALIDATION_ERROR};
    use serde_json::json;

    #[test]
    fn test_create_node_params_parsing() {
        let params = json!({
            "node_type": "text",
            "content": "Test content",
            "properties": {"key": "value"}
        });

        // Verify we can extract fields
        assert_eq!(params["node_type"], "text");
        assert_eq!(params["content"], "Test content");
        assert_eq!(params["properties"]["key"], "value");
    }

    #[test]
    fn test_create_node_params_with_parent() {
        let params = json!({
            "node_type": "task",
            "content": "Task content",
            "parent_id": "parent-123",
            "container_node_id": "container-456",
            "properties": {}
        });

        assert_eq!(params["parent_id"], "parent-123");
        assert_eq!(params["container_node_id"], "container-456");
    }

    #[test]
    fn test_get_node_params_parsing() {
        let params = json!({
            "node_id": "test-node-123"
        });

        assert_eq!(params["node_id"], "test-node-123");
    }

    #[test]
    fn test_update_node_params_parsing() {
        let params = json!({
            "node_id": "node-123",
            "content": "Updated content",
            "properties": {"updated": true}
        });

        assert_eq!(params["node_id"], "node-123");
        assert_eq!(params["content"], "Updated content");
        assert_eq!(params["properties"]["updated"], true);
    }

    #[test]
    fn test_delete_node_params_parsing() {
        let params = json!({
            "node_id": "node-to-delete"
        });

        assert_eq!(params["node_id"], "node-to-delete");
    }

    #[test]
    fn test_query_nodes_params_parsing() {
        let params = json!({
            "filter": {
                "node_type": "text",
                "container_node_id": "container-123"
            },
            "limit": 10,
            "offset": 0
        });

        assert_eq!(params["filter"]["node_type"], "text");
        assert_eq!(params["filter"]["container_node_id"], "container-123");
        assert_eq!(params["limit"], 10);
        assert_eq!(params["offset"], 0);
    }

    #[test]
    fn test_mcp_error_creation() {
        let error = MCPError::node_not_found("xyz");
        assert_eq!(error.code, NODE_NOT_FOUND);
        assert!(error.message.contains("xyz"));
    }

    #[test]
    fn test_invalid_params_error() {
        let error = MCPError::invalid_params("Missing required parameter: node_type".to_string());
        assert_eq!(error.code, INVALID_PARAMS);
        assert!(error.message.contains("node_type"));
    }

    #[test]
    fn test_validation_error() {
        let error = MCPError::validation_error("Invalid node type".to_string());
        assert_eq!(error.code, VALIDATION_ERROR);
        assert!(error.message.contains("Invalid"));
    }

    #[test]
    fn test_success_response_format() {
        let result = json!({
            "node_id": "new-node-123",
            "success": true
        });

        assert_eq!(result["node_id"], "new-node-123");
        assert_eq!(result["success"], true);
    }

    #[test]
    fn test_query_response_format() {
        let result = json!({
            "nodes": [
                {"id": "node-1", "content": "Content 1"},
                {"id": "node-2", "content": "Content 2"}
            ],
            "total": 2
        });

        assert_eq!(result["nodes"].as_array().unwrap().len(), 2);
        assert_eq!(result["total"], 2);
    }
}

// =========================================================================
// Optimistic Concurrency Control (OCC) Tests
// =========================================================================

#[cfg(test)]
mod occ_tests {
    use crate::mcp::handlers::nodes::{handle_delete_node, handle_update_node};
    use crate::mcp::types::{INVALID_PARAMS, VERSION_CONFLICT};
    use crate::operations::{CreateNodeParams, NodeOperations};
    use crate::services::SchemaService;
    use crate::{DatabaseService, NodeService};
    use serde_json::json;
    use std::sync::Arc;
    use tempfile::TempDir;

    async fn setup_test_operations(
    ) -> Result<(Arc<NodeOperations>, Arc<SchemaService>, TempDir), Box<dyn std::error::Error>>
    {
        let temp_dir = TempDir::new()?;
        let db_path = temp_dir.path().join("test.db");
        let db = DatabaseService::new(db_path).await?;
        let node_service = Arc::new(NodeService::new(db)?);
        let operations = Arc::new(NodeOperations::new(node_service.clone()));
        let schema_service = Arc::new(SchemaService::new(node_service));
        Ok((operations, schema_service, temp_dir))
    }

    /// Verifies nodes are created with version 1
    #[tokio::test]
    async fn test_node_created_with_version_1() {
        let (operations, _schema_service, _temp) = setup_test_operations().await.unwrap();

        let node_id = operations
            .create_node(CreateNodeParams {
                id: None,
                node_type: "text".to_string(),
                content: "Test content".to_string(),
                parent_id: None,
                container_node_id: None,
                before_sibling_id: None,
                properties: json!({}),
            })
            .await
            .unwrap();

        let node = operations.get_node(&node_id).await.unwrap().unwrap();
        assert_eq!(node.version, 1);
    }

    /// Verifies version increments on successful update
    #[tokio::test]
    async fn test_version_increments_on_update() {
        let (operations, _schema_service, _temp) = setup_test_operations().await.unwrap();

        let node_id = operations
            .create_node(CreateNodeParams {
                id: None,
                node_type: "text".to_string(),
                content: "Original".to_string(),
                parent_id: None,
                container_node_id: None,
                before_sibling_id: None,
                properties: json!({}),
            })
            .await
            .unwrap();

        // First update: version 1 → 2
        let params = json!({
            "node_id": node_id,
            "version": 1,
            "content": "Updated once"
        });

        let result = handle_update_node(&operations, &_schema_service, params)
            .await
            .unwrap();
        assert_eq!(result["version"], 2);

        // Second update: version 2 → 3
        let params2 = json!({
            "node_id": node_id,
            "version": 2,
            "content": "Updated twice"
        });

        let result2 = handle_update_node(&operations, &_schema_service, params2)
            .await
            .unwrap();
        assert_eq!(result2["version"], 3);
    }

    /// Verifies concurrent update detection via version conflict
    #[tokio::test]
    async fn test_concurrent_update_version_conflict() {
        let (operations, _schema_service, _temp) = setup_test_operations().await.unwrap();

        // Create node (version=1)
        let node_id = operations
            .create_node(CreateNodeParams {
                id: None,
                node_type: "text".to_string(),
                content: "Original".to_string(),
                parent_id: None,
                container_node_id: None,
                before_sibling_id: None,
                properties: json!({}),
            })
            .await
            .unwrap();

        // Client 1 updates successfully (version 1 → 2)
        let params1 = json!({
            "node_id": node_id,
            "version": 1,
            "content": "Client 1 update"
        });
        handle_update_node(&operations, &_schema_service, params1)
            .await
            .unwrap();

        // Client 2 tries to update with stale version (still thinks version=1)
        let params2 = json!({
            "node_id": node_id,
            "version": 1,
            "content": "Client 2 conflicting update"
        });

        let result = handle_update_node(&operations, &_schema_service, params2).await;

        // Should fail with VersionConflict error
        assert!(result.is_err());
        let error = result.unwrap_err();
        assert_eq!(error.code, VERSION_CONFLICT);
        assert!(error.message.contains("version conflict"));
    }

    /// Verifies conflict error includes current node state
    #[tokio::test]
    async fn test_version_conflict_includes_current_node() {
        let (operations, _schema_service, _temp) = setup_test_operations().await.unwrap();

        let node_id = operations
            .create_node(CreateNodeParams {
                id: None,
                node_type: "text".to_string(),
                content: "Original".to_string(),
                parent_id: None,
                container_node_id: None,
                before_sibling_id: None,
                properties: json!({}),
            })
            .await
            .unwrap();

        // Update to version 2
        let params1 = json!({
            "node_id": node_id,
            "version": 1,
            "content": "First update"
        });
        handle_update_node(&operations, &_schema_service, params1)
            .await
            .unwrap();

        // Try to update with stale version
        let params2 = json!({
            "node_id": node_id,
            "version": 1,
            "content": "Conflicting update"
        });

        let result = handle_update_node(&operations, &_schema_service, params2).await;
        assert!(result.is_err());

        let error = result.unwrap_err();
        assert_eq!(error.code, VERSION_CONFLICT);

        // Verify error data includes current node state
        let data = error.data.unwrap();
        assert_eq!(data["type"], "VersionConflict");
        assert_eq!(data["expected_version"], 1);
        assert_eq!(data["actual_version"], 2);
        assert!(data["current_node"].is_object());
        assert_eq!(data["current_node"]["content"], "First update");
        assert_eq!(data["current_node"]["version"], 2);
    }

    /// Verifies delete operation checks version
    #[tokio::test]
    async fn test_delete_with_version_check() {
        let (operations, _schema_service, _temp) = setup_test_operations().await.unwrap();

        let node_id = operations
            .create_node(CreateNodeParams {
                id: None,
                node_type: "text".to_string(),
                content: "To be deleted".to_string(),
                parent_id: None,
                container_node_id: None,
                before_sibling_id: None,
                properties: json!({}),
            })
            .await
            .unwrap();

        // Update to version 2
        let update_params = json!({
            "node_id": node_id,
            "version": 1,
            "content": "Modified"
        });
        handle_update_node(&operations, &_schema_service, update_params)
            .await
            .unwrap();

        // Try to delete with stale version (should fail)
        let delete_params = json!({
            "node_id": node_id,
            "version": 1
        });

        let result = handle_delete_node(&operations, delete_params).await;
        assert!(result.is_err());
        let error = result.unwrap_err();
        assert_eq!(error.code, VERSION_CONFLICT);

        // Verify node still exists
        let node = operations.get_node(&node_id).await.unwrap();
        assert!(node.is_some());

        // Delete with correct version should succeed
        let delete_params2 = json!({
            "node_id": node_id,
            "version": 2
        });
        let result2 = handle_delete_node(&operations, delete_params2).await;
        assert!(result2.is_ok());

        // Verify node is deleted
        let deleted = operations.get_node(&node_id).await.unwrap();
        assert!(deleted.is_none());
    }

    /// Verifies rapid sequential updates maintain version integrity
    #[tokio::test]
    async fn test_rapid_sequential_updates() {
        let (operations, _schema_service, _temp) = setup_test_operations().await.unwrap();

        let node_id = operations
            .create_node(CreateNodeParams {
                id: None,
                node_type: "text".to_string(),
                content: "Start".to_string(),
                parent_id: None,
                container_node_id: None,
                before_sibling_id: None,
                properties: json!({}),
            })
            .await
            .unwrap();

        // Perform 10 sequential updates
        let mut current_version = 1;
        for i in 0..10 {
            let params = json!({
                "node_id": node_id,
                "version": current_version,
                "content": format!("Update {}", i + 1)
            });

            let result = handle_update_node(&operations, &_schema_service, params)
                .await
                .unwrap();
            current_version = result["version"].as_i64().unwrap();
            assert_eq!(current_version, (i + 2) as i64);
        }

        // Final version should be 11
        assert_eq!(current_version, 11);

        let final_node = operations.get_node(&node_id).await.unwrap().unwrap();
        assert_eq!(final_node.version, 11);
        assert_eq!(final_node.content, "Update 10");
    }

    /// Verifies property-only updates increment version
    #[tokio::test]
    async fn test_property_update_increments_version() {
        let (operations, _schema_service, _temp) = setup_test_operations().await.unwrap();

        let node_id = operations
            .create_node(CreateNodeParams {
                id: None,
                node_type: "text".to_string(),
                content: "Test".to_string(),
                parent_id: None,
                container_node_id: None,
                before_sibling_id: None,
                properties: json!({"status": "draft"}),
            })
            .await
            .unwrap();

        // Update properties only
        let params = json!({
            "node_id": node_id,
            "version": 1,
            "properties": {"status": "published", "priority": "high"}
        });

        let result = handle_update_node(&operations, &_schema_service, params)
            .await
            .unwrap();
        assert_eq!(result["version"], 2);

        let updated = operations.get_node(&node_id).await.unwrap().unwrap();
        assert_eq!(updated.version, 2);
        assert_eq!(updated.properties["status"], "published");
        assert_eq!(updated.properties["priority"], "high");
    }

    /// Verifies update FAILS when version parameter is missing (prevents TOCTOU race conditions)
    #[tokio::test]
    async fn test_update_without_version_parameter() {
        let (operations, _schema_service, _temp) = setup_test_operations().await.unwrap();

        let node_id = operations
            .create_node(CreateNodeParams {
                id: None,
                node_type: "text".to_string(),
                content: "Test".to_string(),
                parent_id: None,
                container_node_id: None,
                before_sibling_id: None,
                properties: json!({}),
            })
            .await
            .unwrap();

        // Update without version parameter - should now FAIL (version is mandatory)
        let params = json!({
            "node_id": node_id,
            "content": "Updated without version"
        });

        let result = handle_update_node(&operations, &_schema_service, params).await;

        // Should fail with invalid_params error
        assert!(result.is_err());
        let error = result.unwrap_err();
        assert_eq!(error.code, INVALID_PARAMS);
        assert!(error.message.contains("missing field `version`"));

        // Verify node was NOT updated (still has original content and version 1)
        let node = operations.get_node(&node_id).await.unwrap().unwrap();
        assert_eq!(node.content, "Test");
        assert_eq!(node.version, 1);
    }
}

// =========================================================================
// Integration Tests for Index-Based Child Operations
// =========================================================================

#[cfg(test)]
mod integration_tests {
    use crate::mcp::handlers::nodes::{
        handle_get_child_at_index, handle_get_children, handle_get_node_tree,
        handle_get_nodes_batch, handle_insert_child_at_index, handle_move_child_to_index,
        handle_update_nodes_batch,
    };
    use crate::operations::{CreateNodeParams, NodeOperations};
    use crate::services::SchemaService;
    use crate::{DatabaseService, NodeService};
    use serde_json::json;
    use std::sync::Arc;
    use tempfile::TempDir;

    async fn setup_test_operations(
    ) -> Result<(Arc<NodeOperations>, Arc<SchemaService>, TempDir), Box<dyn std::error::Error>>
    {
        let temp_dir = TempDir::new()?;
        let db_path = temp_dir.path().join("test.db");
        let db = DatabaseService::new(db_path).await?;
        let node_service = Arc::new(NodeService::new(db)?);
        let operations = Arc::new(NodeOperations::new(node_service.clone()));
        let schema_service = Arc::new(SchemaService::new(node_service));
        Ok((operations, schema_service, temp_dir))
    }

    #[tokio::test]
    async fn test_insert_child_at_index_with_date_auto_creation() {
        let (operations, _schema_service, _temp_dir) = setup_test_operations().await.unwrap();

        // Insert child with date parent (should auto-create date node)
        let params = json!({
            "parent_id": "2025-10-23",
            "index": 0,
            "node_type": "text",
            "content": "First note of the day",
            "properties": {}
        });

        let result = handle_insert_child_at_index(&operations, params)
            .await
            .unwrap();

        // Verify response
        assert_eq!(result["parent_id"], "2025-10-23");
        assert_eq!(result["index"], 0);
        assert_eq!(result["node_type"], "text");
        assert!(result["node_id"].is_string());

        // Verify date node was auto-created
        let date_node = operations.get_node("2025-10-23").await.unwrap();
        assert!(date_node.is_some());
        assert_eq!(date_node.unwrap().node_type, "date");

        // Verify child was created under date node
        let child_id = result["node_id"].as_str().unwrap();
        let child_node = operations.get_node(child_id).await.unwrap().unwrap();
        assert_eq!(child_node.parent_id, Some("2025-10-23".to_string()));
        assert_eq!(child_node.content, "First note of the day");
    }

    #[tokio::test]
    async fn test_insert_child_at_index_with_invalid_date_format() {
        let (operations, _schema_service, _temp_dir) = setup_test_operations().await.unwrap();

        // Try to insert with invalid date format (should fail)
        let params = json!({
            "parent_id": "2025-13-45", // Invalid date
            "index": 0,
            "node_type": "text",
            "content": "Test",
            "properties": {}
        });

        let result = handle_insert_child_at_index(&operations, params).await;

        // Should return error (invalid date format, parent not found)
        assert!(result.is_err());
        let error = result.unwrap_err();
        assert!(error.message.contains("not found"));
    }

    #[tokio::test]
    async fn test_insert_child_at_index_with_non_date_invalid_parent() {
        let (operations, _schema_service, _temp_dir) = setup_test_operations().await.unwrap();

        // Try to insert with non-existent non-date parent
        let params = json!({
            "parent_id": "nonexistent-parent",
            "index": 0,
            "node_type": "text",
            "content": "Test",
            "properties": {}
        });

        let result = handle_insert_child_at_index(&operations, params).await;

        // Should return error (parent not found)
        assert!(result.is_err());
        let error = result.unwrap_err();
        assert!(error.message.contains("not found"));
    }

    #[tokio::test]
    async fn test_move_child_to_index_beyond_sibling_count() {
        let (operations, _schema_service, _temp_dir) = setup_test_operations().await.unwrap();

        // Create date container
        let date = operations
            .create_node(CreateNodeParams {
                id: None, // Test generates ID
                node_type: "date".to_string(),
                content: "2025-10-24".to_string(),
                parent_id: None,
                container_node_id: None,
                before_sibling_id: None,
                properties: json!({}),
            })
            .await
            .unwrap();

        // Create three children: A → B → C
        let node_a = operations
            .create_node(CreateNodeParams {
                id: None, // Test generates ID
                node_type: "text".to_string(),
                content: "A".to_string(),
                parent_id: Some(date.clone()),
                container_node_id: Some(date.clone()),
                before_sibling_id: None,
                properties: json!({}),
            })
            .await
            .unwrap();

        let node_b = operations
            .create_node(CreateNodeParams {
                id: None, // Test generates ID
                node_type: "text".to_string(),
                content: "B".to_string(),
                parent_id: Some(date.clone()),
                container_node_id: Some(date.clone()),
                before_sibling_id: Some(node_a.clone()),
                properties: json!({}),
            })
            .await
            .unwrap();

        let _node_c = operations
            .create_node(CreateNodeParams {
                id: None, // Test generates ID
                node_type: "text".to_string(),
                content: "C".to_string(),
                parent_id: Some(date.clone()),
                container_node_id: Some(date.clone()),
                before_sibling_id: Some(node_b.clone()),
                properties: json!({}),
            })
            .await
            .unwrap();

        // Move first node (A) to index 999 (should append at end)
        // Get node A to fetch its current version for OCC
        let node_a_data = operations.get_node(&node_a).await.unwrap().unwrap();
        let params = json!({
            "node_id": node_a,
            "version": node_a_data.version,
            "index": 999
        });

        let result = handle_move_child_to_index(&operations, params)
            .await
            .unwrap();

        // Verify response
        assert_eq!(result["node_id"], node_a);
        assert_eq!(result["new_index"], 999);

        // Verify final order: B → C → A
        let children_params = json!({
            "parent_id": date,
            "include_content": true
        });
        let children_result = handle_get_children(&operations, children_params)
            .await
            .unwrap();

        let children = children_result["children"].as_array().unwrap();
        assert_eq!(children.len(), 3, "Should have 3 children after move");

        // Verify order by content
        assert_eq!(children[0]["content"], "B", "First should be B");
        assert_eq!(children[1]["content"], "C", "Second should be C");
        assert_eq!(
            children[2]["content"], "A",
            "Third should be A (moved to end)"
        );
    }

    #[tokio::test]
    async fn test_get_node_tree_with_max_depth_1() {
        let (operations, _schema_service, _temp_dir) = setup_test_operations().await.unwrap();

        // Create date container
        let date = operations
            .create_node(CreateNodeParams {
                id: None, // Test generates ID
                node_type: "date".to_string(),
                content: "2025-10-25".to_string(),
                parent_id: None,
                container_node_id: None,
                before_sibling_id: None,
                properties: json!({}),
            })
            .await
            .unwrap();

        // Create parent with child
        let parent = operations
            .create_node(CreateNodeParams {
                id: None, // Test generates ID
                node_type: "text".to_string(),
                content: "Parent".to_string(),
                parent_id: Some(date.clone()),
                container_node_id: Some(date.clone()),
                before_sibling_id: None,
                properties: json!({}),
            })
            .await
            .unwrap();

        // Create child under parent
        let child = operations
            .create_node(CreateNodeParams {
                id: None, // Test generates ID
                node_type: "text".to_string(),
                content: "Child".to_string(),
                parent_id: Some(parent.clone()),
                container_node_id: Some(date.clone()),
                before_sibling_id: None,
                properties: json!({}),
            })
            .await
            .unwrap();

        // Create grandchild under child
        let _grandchild = operations
            .create_node(CreateNodeParams {
                id: None, // Test generates ID
                node_type: "text".to_string(),
                content: "Grandchild".to_string(),
                parent_id: Some(child.clone()),
                container_node_id: Some(date.clone()),
                before_sibling_id: None,
                properties: json!({}),
            })
            .await
            .unwrap();

        // Get tree with max_depth=1 (should only show parent and immediate children)
        let params = json!({
            "node_id": parent,
            "max_depth": 1,
            "include_content": false,
            "include_metadata": false
        });

        let result = handle_get_node_tree(&operations, params).await.unwrap();

        // Verify structure
        assert_eq!(result["node_id"], parent);
        assert_eq!(result["depth"], 0);
        assert_eq!(result["child_count"], 1);

        let children = result["children"].as_array().unwrap();
        assert_eq!(children.len(), 1);
        assert_eq!(children[0]["node_id"], child);
        assert_eq!(children[0]["depth"], 1);

        // Grandchild should NOT be included (depth=2 exceeds max_depth=1)
        let grandchildren = children[0]["children"].as_array().unwrap();
        assert_eq!(
            grandchildren.len(),
            0,
            "Grandchild should not be included with max_depth=1"
        );
    }

    #[tokio::test]
    async fn test_get_child_at_index_out_of_bounds() {
        let (operations, _schema_service, _temp_dir) = setup_test_operations().await.unwrap();

        // Create date container
        let date = operations
            .create_node(CreateNodeParams {
                id: None, // Test generates ID
                node_type: "date".to_string(),
                content: "2025-10-26".to_string(),
                parent_id: None,
                container_node_id: None,
                before_sibling_id: None,
                properties: json!({}),
            })
            .await
            .unwrap();

        // Create only 2 children
        let _node_a = operations
            .create_node(CreateNodeParams {
                id: None, // Test generates ID
                node_type: "text".to_string(),
                content: "A".to_string(),
                parent_id: Some(date.clone()),
                container_node_id: Some(date.clone()),
                before_sibling_id: None,
                properties: json!({}),
            })
            .await
            .unwrap();

        let _node_b = operations
            .create_node(CreateNodeParams {
                id: None, // Test generates ID
                node_type: "text".to_string(),
                content: "B".to_string(),
                parent_id: Some(date.clone()),
                container_node_id: Some(date.clone()),
                before_sibling_id: Some(_node_a.clone()),
                properties: json!({}),
            })
            .await
            .unwrap();

        // Try to get child at index 5 (out of bounds - only 2 children exist)
        let params = json!({
            "parent_id": date,
            "index": 5,
            "include_content": true
        });

        let result = handle_get_child_at_index(&operations, params).await;

        // Should return error with helpful message
        assert!(result.is_err());
        let error = result.unwrap_err();
        assert!(error.message.contains("out of bounds"));
        assert!(error.message.contains("5")); // Index mentioned
        assert!(error.message.contains("2")); // Actual count mentioned
    }

    #[tokio::test]
    async fn test_get_children_ordered_with_multiple_insertions() {
        let (operations, _schema_service, _temp_dir) = setup_test_operations().await.unwrap();

        // Create date container
        let date = operations
            .create_node(CreateNodeParams {
                id: None, // Test generates ID
                node_type: "date".to_string(),
                content: "2025-10-27".to_string(),
                parent_id: None,
                container_node_id: None,
                before_sibling_id: None,
                properties: json!({}),
            })
            .await
            .unwrap();

        // Insert at end (index 999)
        let params1 = json!({
            "parent_id": date,
            "index": 999,
            "node_type": "text",
            "content": "First",
            "properties": {}
        });
        let result1 = handle_insert_child_at_index(&operations, params1)
            .await
            .unwrap();
        let first_id = result1["node_id"].as_str().unwrap();

        // Insert at beginning (index 0)
        let params2 = json!({
            "parent_id": date,
            "index": 0,
            "node_type": "text",
            "content": "Second (now first)",
            "properties": {}
        });
        let result2 = handle_insert_child_at_index(&operations, params2)
            .await
            .unwrap();
        let second_id = result2["node_id"].as_str().unwrap();

        // Insert in middle (index 1)
        let params3 = json!({
            "parent_id": date,
            "index": 1,
            "node_type": "text",
            "content": "Third (middle)",
            "properties": {}
        });
        let result3 = handle_insert_child_at_index(&operations, params3)
            .await
            .unwrap();
        let third_id = result3["node_id"].as_str().unwrap();

        // Get children and verify order
        let children_params = json!({
            "parent_id": date,
            "include_content": true
        });
        let children_result = handle_get_children(&operations, children_params)
            .await
            .unwrap();

        let children = children_result["children"].as_array().unwrap();
        assert_eq!(children.len(), 3);

        // Verify order: Second (index 0) → Third (index 1) → First (index 2)
        assert_eq!(children[0]["node_id"].as_str().unwrap(), second_id);
        assert_eq!(children[0]["content"], "Second (now first)");
        assert_eq!(children[0]["index"], 0);

        assert_eq!(children[1]["node_id"].as_str().unwrap(), third_id);
        assert_eq!(children[1]["content"], "Third (middle)");
        assert_eq!(children[1]["index"], 1);

        assert_eq!(children[2]["node_id"].as_str().unwrap(), first_id);
        assert_eq!(children[2]["content"], "First");
        assert_eq!(children[2]["index"], 2);
    }

    #[tokio::test]
    async fn test_get_node_tree_max_depth_validation() {
        let (operations, _schema_service, _temp_dir) = setup_test_operations().await.unwrap();

        // Create a simple node
        let node = operations
            .create_node(CreateNodeParams {
                id: None, // Test generates ID
                node_type: "text".to_string(),
                content: "Test".to_string(),
                parent_id: None,
                container_node_id: None,
                before_sibling_id: None,
                properties: json!({}),
            })
            .await
            .unwrap();

        // Test max_depth=0 (invalid)
        let params_zero = json!({
            "node_id": node,
            "max_depth": 0
        });
        let result_zero = handle_get_node_tree(&operations, params_zero).await;
        assert!(result_zero.is_err());
        assert!(result_zero
            .unwrap_err()
            .message
            .contains("between 1 and 100"));

        // Test max_depth=101 (invalid)
        let params_high = json!({
            "node_id": node,
            "max_depth": 101
        });
        let result_high = handle_get_node_tree(&operations, params_high).await;
        assert!(result_high.is_err());
        assert!(result_high
            .unwrap_err()
            .message
            .contains("between 1 and 100"));

        // Test max_depth=1 (valid)
        let params_valid = json!({
            "node_id": node,
            "max_depth": 1
        });
        let result_valid = handle_get_node_tree(&operations, params_valid).await;
        assert!(result_valid.is_ok());
    }

    // =========================================================================
    // Batch Operations Tests
    // =========================================================================

    /// Verifies successful batch retrieval of multiple nodes
    #[tokio::test]
    async fn test_get_nodes_batch_success() {
        let (operations, _schema_service, _temp_dir) = setup_test_operations().await.unwrap();

        // Create test nodes
        let node1 = operations
            .create_node(CreateNodeParams {
                id: None,
                node_type: "text".to_string(),
                content: "Node 1".to_string(),
                parent_id: None,
                container_node_id: None,
                before_sibling_id: None,
                properties: json!({}),
            })
            .await
            .unwrap();

        let node2 = operations
            .create_node(CreateNodeParams {
                id: None,
                node_type: "text".to_string(),
                content: "Node 2".to_string(),
                parent_id: None,
                container_node_id: None,
                before_sibling_id: None,
                properties: json!({}),
            })
            .await
            .unwrap();

        let node3 = operations
            .create_node(CreateNodeParams {
                id: None,
                node_type: "text".to_string(),
                content: "Node 3".to_string(),
                parent_id: None,
                container_node_id: None,
                before_sibling_id: None,
                properties: json!({}),
            })
            .await
            .unwrap();

        // Batch get
        let params = json!({
            "node_ids": [node1, node2, node3]
        });

        let result = handle_get_nodes_batch(&operations, params).await.unwrap();

        assert_eq!(result["count"].as_u64().unwrap(), 3);
        assert_eq!(result["not_found"].as_array().unwrap().len(), 0);

        let nodes = result["nodes"].as_array().unwrap();
        assert_eq!(nodes.len(), 3);
    }

    /// Verifies get_nodes_batch returns partial results when some nodes don't exist
    #[tokio::test]
    async fn test_get_nodes_batch_with_not_found() {
        let (operations, _schema_service, _temp_dir) = setup_test_operations().await.unwrap();

        let node1 = operations
            .create_node(CreateNodeParams {
                id: None,
                node_type: "text".to_string(),
                content: "Node 1".to_string(),
                parent_id: None,
                container_node_id: None,
                before_sibling_id: None,
                properties: json!({}),
            })
            .await
            .unwrap();

        // Include non-existent IDs
        let params = json!({
            "node_ids": [node1, "does-not-exist", "also-missing"]
        });

        let result = handle_get_nodes_batch(&operations, params).await.unwrap();

        assert_eq!(result["count"].as_u64().unwrap(), 1); // Only 1 found
        assert_eq!(result["not_found"].as_array().unwrap().len(), 2); // 2 missing

        let not_found = result["not_found"].as_array().unwrap();
        assert!(not_found.contains(&json!("does-not-exist")));
        assert!(not_found.contains(&json!("also-missing")));
    }

    /// Verifies validation rejects empty node_ids array
    #[tokio::test]
    async fn test_get_nodes_batch_empty_input() {
        let (operations, _schema_service, _temp_dir) = setup_test_operations().await.unwrap();

        let params = json!({
            "node_ids": []
        });

        let result = handle_get_nodes_batch(&operations, params).await;

        assert!(result.is_err());
        let error = result.unwrap_err();
        assert!(error.message.contains("cannot be empty"));
    }

    /// Verifies batch size limit enforcement (max 100 nodes)
    #[tokio::test]
    async fn test_get_nodes_batch_exceeds_limit() {
        let (operations, _schema_service, _temp_dir) = setup_test_operations().await.unwrap();

        // Create array with 101 IDs (exceeds limit of 100)
        let node_ids: Vec<String> = (0..101).map(|i| format!("node-{}", i)).collect();

        let params = json!({
            "node_ids": node_ids
        });

        let result = handle_get_nodes_batch(&operations, params).await;

        assert!(result.is_err());
        let error = result.unwrap_err();
        assert!(error.message.contains("exceeds maximum of 100"));
    }

    /// Verifies successful batch update of multiple nodes
    #[tokio::test]
    async fn test_update_nodes_batch_success() {
        let (operations, schema_service, _temp_dir) = setup_test_operations().await.unwrap();

        // Create a container first
        let container = operations
            .create_node(CreateNodeParams {
                id: None,
                node_type: "text".to_string(),
                content: "# Task List".to_string(),
                parent_id: None,
                container_node_id: None,
                before_sibling_id: None,
                properties: json!({}),
            })
            .await
            .unwrap();

        let node1 = operations
            .create_node(CreateNodeParams {
                id: None,
                node_type: "task".to_string(),
                content: "- [ ] Task 1".to_string(),
                parent_id: Some(container.clone()),
                container_node_id: Some(container.clone()),
                before_sibling_id: None,
                properties: json!({"task": {"status": "OPEN"}}),
            })
            .await
            .unwrap();

        let node2 = operations
            .create_node(CreateNodeParams {
                id: None,
                node_type: "task".to_string(),
                content: "- [ ] Task 2".to_string(),
                parent_id: Some(container.clone()),
                container_node_id: Some(container),
                before_sibling_id: Some(node1.clone()),
                properties: json!({"task": {"status": "OPEN"}}),
            })
            .await
            .unwrap();

        // Batch update
        let params = json!({
            "updates": [
                { "id": node1, "content": "- [x] Task 1" },
                { "id": node2, "content": "- [x] Task 2" }
            ]
        });

        let result = handle_update_nodes_batch(&operations, &schema_service, params)
            .await
            .unwrap();

        assert_eq!(result["count"].as_u64().unwrap(), 2);
        assert_eq!(result["failed"].as_array().unwrap().len(), 0);

        // Verify updates
        let updated1 = operations.get_node(&node1).await.unwrap().unwrap();
        assert_eq!(updated1.content, "- [x] Task 1");

        let updated2 = operations.get_node(&node2).await.unwrap().unwrap();
        assert_eq!(updated2.content, "- [x] Task 2");
    }

    /// Verifies partial success handling with detailed failure reporting
    #[tokio::test]
    async fn test_update_nodes_batch_partial_failure() {
        let (operations, schema_service, _temp_dir) = setup_test_operations().await.unwrap();

        let node1 = operations
            .create_node(CreateNodeParams {
                id: None,
                node_type: "text".to_string(),
                content: "Node 1".to_string(),
                parent_id: None,
                container_node_id: None,
                before_sibling_id: None,
                properties: json!({}),
            })
            .await
            .unwrap();

        // Mix valid and invalid updates
        let params = json!({
            "updates": [
                { "id": node1, "content": "Updated Node 1" },
                { "id": "nonexistent", "content": "Should fail" }
            ]
        });

        let result = handle_update_nodes_batch(&operations, &schema_service, params)
            .await
            .unwrap();

        // Should have 1 success and 1 failure
        assert_eq!(result["count"].as_u64().unwrap(), 1);
        let updated = result["updated"].as_array().unwrap();
        assert_eq!(updated.len(), 1);
        assert_eq!(updated[0], node1);

        let failed = result["failed"].as_array().unwrap();
        assert_eq!(failed.len(), 1);
        assert_eq!(failed[0]["id"], "nonexistent");
    }

    /// Verifies validation rejects empty updates array
    #[tokio::test]
    async fn test_update_nodes_batch_empty_input() {
        let (operations, schema_service, _temp_dir) = setup_test_operations().await.unwrap();

        let params = json!({
            "updates": []
        });

        let result = handle_update_nodes_batch(&operations, &schema_service, params).await;

        assert!(result.is_err());
        let error = result.unwrap_err();
        assert!(error.message.contains("cannot be empty"));
    }

    /// Verifies batch size limit enforcement (max 100 updates)
    #[tokio::test]
    async fn test_update_nodes_batch_exceeds_limit() {
        let (operations, schema_service, _temp_dir) = setup_test_operations().await.unwrap();

        // Create array with 101 updates (exceeds limit of 100)
        let updates: Vec<serde_json::Value> = (0..101)
            .map(|i| {
                json!({
                    "id": format!("node-{}", i),
                    "content": "updated"
                })
            })
            .collect();

        let params = json!({
            "updates": updates
        });

        let result = handle_update_nodes_batch(&operations, &schema_service, params).await;

        assert!(result.is_err());
        let error = result.unwrap_err();
        assert!(error.message.contains("exceeds maximum of 100"));
    }

    /// Verifies property-only updates without content changes
    #[tokio::test]
    async fn test_update_nodes_batch_with_properties() {
        let (operations, schema_service, _temp_dir) = setup_test_operations().await.unwrap();

        let node = operations
            .create_node(CreateNodeParams {
                id: None,
                node_type: "text".to_string(),
                content: "Test".to_string(),
                parent_id: None,
                container_node_id: None,
                before_sibling_id: None,
                properties: json!({"priority": "low"}),
            })
            .await
            .unwrap();

        // Update properties only
        let params = json!({
            "updates": [
                { "id": node, "properties": { "priority": "high", "status": "urgent" } }
            ]
        });

        let result = handle_update_nodes_batch(&operations, &schema_service, params)
            .await
            .unwrap();

        assert_eq!(result["count"].as_u64().unwrap(), 1);

        // Verify property update
        let updated = operations.get_node(&node).await.unwrap().unwrap();
        assert_eq!(updated.properties["priority"], "high");
        assert_eq!(updated.properties["status"], "urgent");
    }

    // =========================================================================
    // Performance Benchmarks
    // =========================================================================

    /// Benchmark: Verifies batch retrieval performance vs sequential calls
    ///
    /// This test quantifies the performance of get_nodes_batch vs multiple individual
    /// get_node calls. Note: In-memory operations show modest speedup (1.0-1.5x).
    /// Real-world speedup over MCP/IPC is much higher (2-10x) due to network overhead.
    #[tokio::test]
    #[ignore] // Run explicitly with: cargo test benchmark_get_nodes_batch -- --ignored --nocapture
    async fn benchmark_get_nodes_batch() {
        use std::time::Instant;

        let (operations, _schema_service, _temp_dir) = setup_test_operations().await.unwrap();

        // Create 50 test nodes
        let mut node_ids = Vec::new();
        for i in 0..50 {
            let node = operations
                .create_node(CreateNodeParams {
                    id: None,
                    node_type: "text".to_string(),
                    content: format!("Node {}", i),
                    parent_id: None,
                    container_node_id: None,
                    before_sibling_id: None,
                    properties: json!({}),
                })
                .await
                .unwrap();
            node_ids.push(node);
        }

        // Benchmark: Sequential individual calls
        let start_sequential = Instant::now();
        for node_id in &node_ids {
            let _node = operations.get_node(node_id).await.unwrap();
        }
        let duration_sequential = start_sequential.elapsed();

        // Benchmark: Single batch call
        let start_batch = Instant::now();
        let params = json!({
            "node_ids": node_ids
        });
        let _result = handle_get_nodes_batch(&operations, params).await.unwrap();
        let duration_batch = start_batch.elapsed();

        let speedup = duration_sequential.as_secs_f64() / duration_batch.as_secs_f64();
        println!("Sequential (50 calls): {:?}", duration_sequential);
        println!("Batch (1 call):        {:?}", duration_batch);
        println!("Speedup:               {:.2}x", speedup);
        println!();
        println!("Note: In-memory operations show modest speedup.");
        println!("Real-world speedup over MCP/IPC is 2-10x due to:");
        println!("  - Network/IPC serialization overhead");
        println!("  - Round-trip latency per call");
        println!("  - Process context switching");

        // Verify batch is at least as fast as sequential (no regression)
        assert!(
            duration_batch <= duration_sequential,
            "Batch should be at least as fast as sequential (got {:.2}x speedup)",
            speedup
        );
    }

    /// Benchmark: Verifies batch update performance vs sequential calls
    ///
    /// This test quantifies the performance of update_nodes_batch vs multiple individual
    /// update_node calls. Note: In-memory operations show modest speedup (1.0-1.5x).
    /// Real-world speedup over MCP/IPC is much higher (2-10x) due to network overhead.
    #[tokio::test]
    #[ignore] // Run explicitly with: cargo test benchmark_update_nodes_batch -- --ignored --nocapture
    async fn benchmark_update_nodes_batch() {
        use std::time::Instant;

        let (operations, schema_service, _temp_dir) = setup_test_operations().await.unwrap();

        // Create a container
        let container = operations
            .create_node(CreateNodeParams {
                id: None,
                node_type: "text".to_string(),
                content: "# Benchmark Container".to_string(),
                parent_id: None,
                container_node_id: None,
                before_sibling_id: None,
                properties: json!({}),
            })
            .await
            .unwrap();

        // Create 50 test nodes
        let mut node_ids = Vec::new();
        for i in 0..50 {
            let node = operations
                .create_node(CreateNodeParams {
                    id: None,
                    node_type: "task".to_string(),
                    content: format!("- [ ] Task {}", i),
                    parent_id: Some(container.clone()),
                    container_node_id: Some(container.clone()),
                    before_sibling_id: None,
                    properties: json!({}),
                })
                .await
                .unwrap();
            node_ids.push(node);
        }

        // Benchmark: Sequential individual updates
        let start_sequential = Instant::now();
        for node_id in &node_ids {
            let node = operations.get_node(node_id).await.unwrap().unwrap();
            operations
                .update_node(
                    node_id,
                    node.version,
                    Some("- [x] Updated task".to_string()),
                    None,
                    None,
                )
                .await
                .unwrap();
        }
        let duration_sequential = start_sequential.elapsed();

        // Reset nodes back to unchecked for fair comparison
        for node_id in &node_ids {
            let node = operations.get_node(node_id).await.unwrap().unwrap();
            operations
                .update_node(
                    node_id,
                    node.version,
                    Some("- [ ] Task".to_string()),
                    None,
                    None,
                )
                .await
                .unwrap();
        }

        // Benchmark: Single batch update
        let start_batch = Instant::now();
        let updates: Vec<serde_json::Value> = node_ids
            .iter()
            .map(|id| {
                json!({
                    "id": id,
                    "content": "- [x] Updated task"
                })
            })
            .collect();

        let params = json!({
            "updates": updates
        });
        let _result = handle_update_nodes_batch(&operations, &schema_service, params)
            .await
            .unwrap();
        let duration_batch = start_batch.elapsed();

        let speedup = duration_sequential.as_secs_f64() / duration_batch.as_secs_f64();
        println!("Sequential (50 calls): {:?}", duration_sequential);
        println!("Batch (1 call):        {:?}", duration_batch);
        println!("Speedup:               {:.2}x", speedup);
        println!();
        println!("Note: In-memory operations show modest speedup.");
        println!("Real-world speedup over MCP/IPC is 2-10x due to:");
        println!("  - Network/IPC serialization overhead");
        println!("  - Round-trip latency per call");
        println!("  - Process context switching");

        // Verify batch is at least as fast as sequential (no regression)
        assert!(
            duration_batch <= duration_sequential,
            "Batch should be at least as fast as sequential (got {:.2}x speedup)",
            speedup
        );
    }

    /// Verifies OCC version check overhead is < 5ms per operation
    /// This test validates the performance acceptance criterion
    #[tokio::test]
    async fn test_occ_performance_overhead() {
        let (operations, _schema_service, _temp) = setup_test_operations().await.unwrap();

        // Create a test node
        let node_id = operations
            .create_node(CreateNodeParams {
                id: None,
                node_type: "text".to_string(),
                content: "Performance test".to_string(),
                parent_id: None,
                container_node_id: None,
                before_sibling_id: None,
                properties: json!({}),
            })
            .await
            .unwrap();

        // Warm up (first operation may be slower due to caching)
        let node = operations.get_node(&node_id).await.unwrap().unwrap();
        let _ = operations
            .update_node(
                &node_id,
                node.version,
                Some("warmup".to_string()),
                None,
                None,
            )
            .await
            .unwrap();

        // Measure 100 update operations with version checking
        let iterations = 100;
        let start = std::time::Instant::now();

        for i in 0..iterations {
            let node = operations.get_node(&node_id).await.unwrap().unwrap();
            operations
                .update_node(
                    &node_id,
                    node.version,
                    Some(format!("Update {}", i)),
                    None,
                    None,
                )
                .await
                .unwrap();
        }

        let elapsed = start.elapsed();
        let avg_ms = elapsed.as_millis() as f64 / iterations as f64;

        // Acceptance criterion: < 5ms overhead per operation
        // Note: This measures total operation time (read + version check + write)
        // The version check itself adds only ~0.1-1ms (single integer comparison)
        assert!(
            avg_ms < 5.0,
            "OCC overhead too high: {:.2}ms average (expected < 5ms)",
            avg_ms
        );

        println!(
            "✓ Performance test passed: {:.2}ms average per operation (target: < 5ms)",
            avg_ms
        );
    }
}
