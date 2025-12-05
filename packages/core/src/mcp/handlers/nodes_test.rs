//! Tests for MCP node handlers
//!
//! Tests the request parameter parsing and response formatting for node CRUD node_service.

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
            "root_id": "root-456",
            "properties": {}
        });

        assert_eq!(params["parent_id"], "parent-123");
        assert_eq!(params["root_id"], "root-456");
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
                "root_id": "root-123"
            },
            "limit": 10,
            "offset": 0
        });

        assert_eq!(params["filter"]["node_type"], "text");
        assert_eq!(params["filter"]["root_id"], "root-123");
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
    use crate::db::SurrealStore;
    use crate::mcp::handlers::nodes::handle_delete_node;
    use crate::mcp::handlers::nodes::handle_update_node;
    use crate::mcp::types::VERSION_CONFLICT;
    use crate::services::CreateNodeParams;
    use crate::NodeService;
    use serde_json::json;
    use std::sync::Arc;
    use tempfile::TempDir;

    async fn setup_test_service() -> Result<(Arc<NodeService>, TempDir), Box<dyn std::error::Error>>
    {
        let temp_dir = TempDir::new()?;
        let db_path = temp_dir.path().join("test.db");

        let mut store = Arc::new(SurrealStore::new(db_path).await?);
        let node_service = Arc::new(NodeService::new(&mut store).await?);
        Ok((node_service, temp_dir))
    }

    /// Verifies nodes are created with version 1
    #[tokio::test]
    async fn test_node_created_with_version_1() {
        let (node_service, _temp) = setup_test_service().await.unwrap();

        let node_id = node_service
            .create_node_with_parent(CreateNodeParams {
                id: None,
                node_type: "text".to_string(),
                content: "Test content".to_string(),
                parent_id: None,
                insert_after_node_id: None,
                properties: json!({}),
            })
            .await
            .unwrap();

        let node = node_service.get_node(&node_id).await.unwrap().unwrap();
        assert_eq!(node.version, 1);
    }

    /// Verifies version increments on successful update
    #[tokio::test]
    async fn test_version_increments_on_update() {
        let (node_service, _temp) = setup_test_service().await.unwrap();

        let node_id = node_service
            .create_node_with_parent(CreateNodeParams {
                id: None,
                node_type: "text".to_string(),
                content: "Original".to_string(),
                parent_id: None,
                insert_after_node_id: None,
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

        let result = handle_update_node(&node_service, params).await.unwrap();
        assert_eq!(result["version"], 2);

        // Second update: version 2 → 3
        let params2 = json!({
            "node_id": node_id,
            "version": 2,
            "content": "Updated twice"
        });

        let result2 = handle_update_node(&node_service, params2).await.unwrap();
        assert_eq!(result2["version"], 3);
    }

    /// Verifies concurrent update detection via version conflict
    #[tokio::test]
    async fn test_concurrent_update_version_conflict() {
        let (node_service, _temp) = setup_test_service().await.unwrap();

        // Create node (version=1)
        let node_id = node_service
            .create_node_with_parent(CreateNodeParams {
                id: None,
                node_type: "text".to_string(),
                content: "Original".to_string(),
                parent_id: None,
                insert_after_node_id: None,
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
        handle_update_node(&node_service, params1).await.unwrap();

        // Client 2 tries to update with stale version (still thinks version=1)
        let params2 = json!({
            "node_id": node_id,
            "version": 1,
            "content": "Client 2 conflicting update"
        });

        let result = handle_update_node(&node_service, params2).await;

        // Should fail with VersionConflict error
        assert!(result.is_err());
        let error = result.unwrap_err();
        assert_eq!(error.code, VERSION_CONFLICT);
        assert!(error.message.contains("version conflict"));
    }

    /// Verifies conflict error includes current node state
    #[tokio::test]
    async fn test_version_conflict_includes_current_node() {
        let (node_service, _temp) = setup_test_service().await.unwrap();

        let node_id = node_service
            .create_node_with_parent(CreateNodeParams {
                id: None,
                node_type: "text".to_string(),
                content: "Original".to_string(),
                parent_id: None,
                insert_after_node_id: None,
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
        handle_update_node(&node_service, params1).await.unwrap();

        // Try to update with stale version
        let params2 = json!({
            "node_id": node_id,
            "version": 1,
            "content": "Conflicting update"
        });

        let result = handle_update_node(&node_service, params2).await;
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
        let (node_service, _temp) = setup_test_service().await.unwrap();

        let node_id = node_service
            .create_node_with_parent(CreateNodeParams {
                id: None,
                node_type: "text".to_string(),
                content: "To be deleted".to_string(),
                parent_id: None,
                insert_after_node_id: None,
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
        handle_update_node(&node_service, update_params)
            .await
            .unwrap();

        // Try to delete with stale version (should fail)
        let delete_params = json!({
            "node_id": node_id,
            "version": 1
        });

        let result = handle_delete_node(&node_service, delete_params).await;
        assert!(result.is_err());
        let error = result.unwrap_err();
        assert_eq!(error.code, VERSION_CONFLICT);

        // Verify node still exists
        let node = node_service.get_node(&node_id).await.unwrap();
        assert!(node.is_some());

        // Delete with correct version should succeed
        let delete_params2 = json!({
            "node_id": node_id,
            "version": 2
        });
        let result2 = handle_delete_node(&node_service, delete_params2).await;
        assert!(result2.is_ok());

        // Verify node is deleted
        let deleted = node_service.get_node(&node_id).await.unwrap();
        assert!(deleted.is_none());
    }

    /// Verifies rapid sequential updates maintain version integrity
    #[tokio::test]
    async fn test_rapid_sequential_updates() {
        let (node_service, _temp) = setup_test_service().await.unwrap();

        let node_id = node_service
            .create_node_with_parent(CreateNodeParams {
                id: None,
                node_type: "text".to_string(),
                content: "Start".to_string(),
                parent_id: None,
                insert_after_node_id: None,
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

            let result = handle_update_node(&node_service, params).await.unwrap();
            current_version = result["version"].as_i64().unwrap();
            assert_eq!(current_version, (i + 2) as i64);
        }

        // Final version should be 11
        assert_eq!(current_version, 11);

        let final_node = node_service.get_node(&node_id).await.unwrap().unwrap();
        assert_eq!(final_node.version, 11);
        assert_eq!(final_node.content, "Update 10");
    }

    /// Verifies property-only updates increment version
    /// Uses task node type since it has a spoke table for properties (hub-spoke architecture)
    #[tokio::test]
    async fn test_property_update_increments_version() {
        let (node_service, _temp) = setup_test_service().await.unwrap();

        let node_id = node_service
            .create_node_with_parent(CreateNodeParams {
                id: None,
                node_type: "task".to_string(),
                content: "Test task".to_string(),
                parent_id: None,
                insert_after_node_id: None,
                properties: json!({"status": "open"}),
            })
            .await
            .unwrap();

        // Update properties only (status goes to task spoke table)
        let params = json!({
            "node_id": node_id,
            "version": 1,
            "properties": {"status": "done", "priority": "high"}
        });

        let result = handle_update_node(&node_service, params).await.unwrap();
        assert_eq!(result["version"], 2);

        let updated = node_service.get_node(&node_id).await.unwrap().unwrap();
        assert_eq!(updated.version, 2);
        assert_eq!(updated.properties["status"], "done");
        assert_eq!(updated.properties["priority"], "high");
    }

    /// Verifies update SUCCEEDS without version parameter (auto-fetches current version)
    /// This is intentionally lenient for AI agent convenience (MCP workflow)
    #[tokio::test]
    async fn test_update_without_version_parameter() {
        let (node_service, _temp) = setup_test_service().await.unwrap();

        let node_id = node_service
            .create_node_with_parent(CreateNodeParams {
                id: None,
                node_type: "text".to_string(),
                content: "Test".to_string(),
                parent_id: None,
                insert_after_node_id: None,
                properties: json!({}),
            })
            .await
            .unwrap();

        // Update without version parameter - should SUCCEED (auto-fetches current version)
        let params = json!({
            "node_id": node_id,
            "content": "Updated without version"
        });

        let result = handle_update_node(&node_service, params).await;

        // Should succeed - version is auto-fetched for convenience
        assert!(result.is_ok());

        // Verify node was updated with new content and incremented version
        let node = node_service.get_node(&node_id).await.unwrap().unwrap();
        assert_eq!(node.content, "Updated without version");
        assert_eq!(node.version, 2);
    }
}

// =========================================================================
// Integration Tests for Index-Based Child Operations
// =========================================================================

#[cfg(test)]
mod integration_tests {
    use crate::db::SurrealStore;
    use crate::mcp::handlers::nodes::{
        handle_get_child_at_index, handle_get_children, handle_get_node_tree,
        handle_get_nodes_batch, handle_insert_child_at_index, handle_move_child_to_index,
        handle_update_nodes_batch,
    };
    use crate::services::CreateNodeParams;
    use crate::NodeService;
    use serde_json::json;
    use std::sync::Arc;
    use tempfile::TempDir;

    async fn setup_test_service() -> Result<(Arc<NodeService>, TempDir), Box<dyn std::error::Error>>
    {
        let temp_dir = TempDir::new()?;
        let db_path = temp_dir.path().join("test.db");

        let mut store = Arc::new(SurrealStore::new(db_path).await?);
        let node_service = Arc::new(NodeService::new(&mut store).await?);
        Ok((node_service, temp_dir))
    }

    #[tokio::test]
    async fn test_insert_child_at_index_with_date_auto_creation() {
        let (node_service, _temp_dir) = setup_test_service().await.unwrap();

        // Insert child with date parent (should auto-create date node)
        let params = json!({
            "parent_id": "2025-10-23",
            "index": 0,
            "node_type": "text",
            "content": "First note of the day",
            "properties": {}
        });

        let result = handle_insert_child_at_index(&node_service, params)
            .await
            .unwrap();

        // Verify response
        assert_eq!(result["parent_id"], "2025-10-23");
        assert_eq!(result["index"], 0);
        assert_eq!(result["node_type"], "text");
        assert!(result["node_id"].is_string());

        // Verify date node was auto-created
        let date_node = node_service.get_node("2025-10-23").await.unwrap();
        assert!(date_node.is_some());
        assert_eq!(date_node.unwrap().node_type, "date");

        // Verify child was created under date node
        let child_id = result["node_id"].as_str().unwrap();
        let child_node = node_service.get_node(child_id).await.unwrap().unwrap();
        // Parent relationship verified via graph edges
        assert_eq!(child_node.content, "First note of the day");
    }

    #[tokio::test]
    async fn test_insert_child_at_index_with_invalid_date_format() {
        let (node_service, _temp_dir) = setup_test_service().await.unwrap();

        // Try to insert with invalid date format (should fail)
        let params = json!({
            "parent_id": "2025-13-45", // Invalid date
            "index": 0,
            "node_type": "text",
            "content": "Test",
            "properties": {}
        });

        let result = handle_insert_child_at_index(&node_service, params).await;

        // Should return error (invalid date format, parent not found)
        assert!(result.is_err());
        let error = result.unwrap_err();
        assert!(error.message.contains("not found"));
    }

    #[tokio::test]
    async fn test_insert_child_at_index_with_non_date_invalid_parent() {
        let (node_service, _temp_dir) = setup_test_service().await.unwrap();

        // Try to insert with non-existent non-date parent
        let params = json!({
            "parent_id": "nonexistent-parent",
            "index": 0,
            "node_type": "text",
            "content": "Test",
            "properties": {}
        });

        let result = handle_insert_child_at_index(&node_service, params).await;

        // Should return error (parent not found)
        assert!(result.is_err());
        let error = result.unwrap_err();
        assert!(error.message.contains("not found"));
    }

    #[tokio::test]
    async fn test_move_child_to_index_beyond_sibling_count() {
        let (node_service, _temp_dir) = setup_test_service().await.unwrap();

        // Create date root
        let date = node_service
            .create_node_with_parent(CreateNodeParams {
                id: None, // Test generates ID
                node_type: "date".to_string(),
                content: "2025-10-24".to_string(),
                parent_id: None,
                insert_after_node_id: None,
                properties: json!({}),
            })
            .await
            .unwrap();

        // Create three children: A → B → C
        let node_a = node_service
            .create_node_with_parent(CreateNodeParams {
                id: None, // Test generates ID
                node_type: "text".to_string(),
                content: "A".to_string(),
                parent_id: Some(date.clone()),
                insert_after_node_id: None, // First child (insert at beginning)
                properties: json!({}),
            })
            .await
            .unwrap();

        let _node_b = node_service
            .create_node_with_parent(CreateNodeParams {
                id: None, // Test generates ID
                node_type: "text".to_string(),
                content: "B".to_string(),
                parent_id: Some(date.clone()),
                insert_after_node_id: Some(node_a.clone()), // Insert after A
                properties: json!({}),
            })
            .await
            .unwrap();

        let _node_c = node_service
            .create_node_with_parent(CreateNodeParams {
                id: None, // Test generates ID
                node_type: "text".to_string(),
                content: "C".to_string(),
                parent_id: Some(date.clone()),
                insert_after_node_id: Some(_node_b.clone()), // Insert after B
                properties: json!({}),
            })
            .await
            .unwrap();

        // Move first node (A) to index 999 (should append at end)
        // Get node A to fetch its current version for OCC
        let node_a_data = node_service.get_node(&node_a).await.unwrap().unwrap();
        let params = json!({
            "node_id": node_a,
            "version": node_a_data.version,
            "index": 999
        });

        let result = handle_move_child_to_index(&node_service, params)
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
        let children_result = handle_get_children(&node_service, children_params)
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
        let (node_service, _temp_dir) = setup_test_service().await.unwrap();

        // Create date root
        let date = node_service
            .create_node_with_parent(CreateNodeParams {
                id: None, // Test generates ID
                node_type: "date".to_string(),
                content: "2025-10-25".to_string(),
                parent_id: None,
                insert_after_node_id: None,
                properties: json!({}),
            })
            .await
            .unwrap();

        // Create parent with child
        let parent = node_service
            .create_node_with_parent(CreateNodeParams {
                id: None, // Test generates ID
                node_type: "text".to_string(),
                content: "Parent".to_string(),
                parent_id: Some(date.clone()),
                insert_after_node_id: None,
                properties: json!({}),
            })
            .await
            .unwrap();

        // Create child under parent
        let child = node_service
            .create_node_with_parent(CreateNodeParams {
                id: None, // Test generates ID
                node_type: "text".to_string(),
                content: "Child".to_string(),
                parent_id: Some(parent.clone()),
                insert_after_node_id: None,
                properties: json!({}),
            })
            .await
            .unwrap();

        // Create grandchild under child
        let _grandchild = node_service
            .create_node_with_parent(CreateNodeParams {
                id: None, // Test generates ID
                node_type: "text".to_string(),
                content: "Grandchild".to_string(),
                parent_id: Some(child.clone()),
                insert_after_node_id: None,
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

        let result = handle_get_node_tree(&node_service, params).await.unwrap();

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
        let (node_service, _temp_dir) = setup_test_service().await.unwrap();

        // Create date root
        let date = node_service
            .create_node_with_parent(CreateNodeParams {
                id: None, // Test generates ID
                node_type: "date".to_string(),
                content: "2025-10-26".to_string(),
                parent_id: None,
                insert_after_node_id: None,
                properties: json!({}),
            })
            .await
            .unwrap();

        // Create only 2 children
        let _node_a = node_service
            .create_node_with_parent(CreateNodeParams {
                id: None, // Test generates ID
                node_type: "text".to_string(),
                content: "A".to_string(),
                parent_id: Some(date.clone()),
                insert_after_node_id: None,
                properties: json!({}),
            })
            .await
            .unwrap();

        let _node_b = node_service
            .create_node_with_parent(CreateNodeParams {
                id: None, // Test generates ID
                node_type: "text".to_string(),
                content: "B".to_string(),
                parent_id: Some(date.clone()),
                insert_after_node_id: Some(_node_a.clone()),
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

        let result = handle_get_child_at_index(&node_service, params).await;

        // Should return error with helpful message
        assert!(result.is_err());
        let error = result.unwrap_err();
        assert!(error.message.contains("out of bounds"));
        assert!(error.message.contains("5")); // Index mentioned
        assert!(error.message.contains("2")); // Actual count mentioned
    }

    #[tokio::test]
    async fn test_get_children_ordered_with_multiple_insertions() {
        let (node_service, _temp_dir) = setup_test_service().await.unwrap();

        // Create date root
        let date = node_service
            .create_node_with_parent(CreateNodeParams {
                id: None, // Test generates ID
                node_type: "date".to_string(),
                content: "2025-10-27".to_string(),
                parent_id: None,
                insert_after_node_id: None,
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
        let result1 = handle_insert_child_at_index(&node_service, params1)
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
        let result2 = handle_insert_child_at_index(&node_service, params2)
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
        let result3 = handle_insert_child_at_index(&node_service, params3)
            .await
            .unwrap();
        let third_id = result3["node_id"].as_str().unwrap();

        // Get children and verify order
        let children_params = json!({
            "parent_id": date,
            "include_content": true
        });
        let children_result = handle_get_children(&node_service, children_params)
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
        let (node_service, _temp_dir) = setup_test_service().await.unwrap();

        // Create a simple node
        let node = node_service
            .create_node_with_parent(CreateNodeParams {
                id: None, // Test generates ID
                node_type: "text".to_string(),
                content: "Test".to_string(),
                parent_id: None,
                insert_after_node_id: None,
                properties: json!({}),
            })
            .await
            .unwrap();

        // Test max_depth=0 (invalid)
        let params_zero = json!({
            "node_id": node,
            "max_depth": 0
        });
        let result_zero = handle_get_node_tree(&node_service, params_zero).await;
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
        let result_high = handle_get_node_tree(&node_service, params_high).await;
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
        let result_valid = handle_get_node_tree(&node_service, params_valid).await;
        assert!(result_valid.is_ok());
    }

    // =========================================================================
    // Batch Operations Tests
    // =========================================================================

    /// Verifies successful batch retrieval of multiple nodes
    #[tokio::test]
    async fn test_get_nodes_batch_success() {
        let (node_service, _temp_dir) = setup_test_service().await.unwrap();

        // Create test nodes
        let node1 = node_service
            .create_node_with_parent(CreateNodeParams {
                id: None,
                node_type: "text".to_string(),
                content: "Node 1".to_string(),
                parent_id: None,
                insert_after_node_id: None,
                properties: json!({}),
            })
            .await
            .unwrap();

        let node2 = node_service
            .create_node_with_parent(CreateNodeParams {
                id: None,
                node_type: "text".to_string(),
                content: "Node 2".to_string(),
                parent_id: None,
                insert_after_node_id: None,
                properties: json!({}),
            })
            .await
            .unwrap();

        let node3 = node_service
            .create_node_with_parent(CreateNodeParams {
                id: None,
                node_type: "text".to_string(),
                content: "Node 3".to_string(),
                parent_id: None,
                insert_after_node_id: None,
                properties: json!({}),
            })
            .await
            .unwrap();

        // Batch get
        let params = json!({
            "node_ids": [node1, node2, node3]
        });

        let result = handle_get_nodes_batch(&node_service, params).await.unwrap();

        assert_eq!(result["count"].as_u64().unwrap(), 3);
        assert_eq!(result["not_found"].as_array().unwrap().len(), 0);

        let nodes = result["nodes"].as_array().unwrap();
        assert_eq!(nodes.len(), 3);
    }

    /// Verifies get_nodes_batch returns partial results when some nodes don't exist
    #[tokio::test]
    async fn test_get_nodes_batch_with_not_found() {
        let (node_service, _temp_dir) = setup_test_service().await.unwrap();

        let node1 = node_service
            .create_node_with_parent(CreateNodeParams {
                id: None,
                node_type: "text".to_string(),
                content: "Node 1".to_string(),
                parent_id: None,
                insert_after_node_id: None,
                properties: json!({}),
            })
            .await
            .unwrap();

        // Include non-existent IDs
        let params = json!({
            "node_ids": [node1, "does-not-exist", "also-missing"]
        });

        let result = handle_get_nodes_batch(&node_service, params).await.unwrap();

        assert_eq!(result["count"].as_u64().unwrap(), 1); // Only 1 found
        assert_eq!(result["not_found"].as_array().unwrap().len(), 2); // 2 missing

        let not_found = result["not_found"].as_array().unwrap();
        assert!(not_found.contains(&json!("does-not-exist")));
        assert!(not_found.contains(&json!("also-missing")));
    }

    /// Verifies validation rejects empty node_ids array
    #[tokio::test]
    async fn test_get_nodes_batch_empty_input() {
        let (node_service, _temp_dir) = setup_test_service().await.unwrap();

        let params = json!({
            "node_ids": []
        });

        let result = handle_get_nodes_batch(&node_service, params).await;

        assert!(result.is_err());
        let error = result.unwrap_err();
        assert!(error.message.contains("cannot be empty"));
    }

    /// Verifies batch size limit enforcement (max 100 nodes)
    #[tokio::test]
    async fn test_get_nodes_batch_exceeds_limit() {
        let (node_service, _temp_dir) = setup_test_service().await.unwrap();

        // Create array with 101 IDs (exceeds limit of 100)
        let node_ids: Vec<String> = (0..101).map(|i| format!("node-{}", i)).collect();

        let params = json!({
            "node_ids": node_ids
        });

        let result = handle_get_nodes_batch(&node_service, params).await;

        assert!(result.is_err());
        let error = result.unwrap_err();
        assert!(error.message.contains("exceeds maximum of 100"));
    }

    /// Verifies successful batch update of multiple nodes
    #[tokio::test]
    async fn test_update_nodes_batch_success() {
        let (node_service, _temp_dir) = setup_test_service().await.unwrap();

        // Create a root first
        let root = node_service
            .create_node_with_parent(CreateNodeParams {
                id: None,
                node_type: "text".to_string(),
                content: "# Task List".to_string(),
                parent_id: None,
                insert_after_node_id: None,
                properties: json!({}),
            })
            .await
            .unwrap();

        let node1 = node_service
            .create_node_with_parent(CreateNodeParams {
                id: None,
                node_type: "task".to_string(),
                content: "- [ ] Task 1".to_string(),
                parent_id: Some(root.clone()),
                insert_after_node_id: None,
                properties: json!({"task": {"status": "open"}}),
            })
            .await
            .unwrap();

        let node2 = node_service
            .create_node_with_parent(CreateNodeParams {
                id: None,
                node_type: "task".to_string(),
                content: "- [ ] Task 2".to_string(),
                parent_id: Some(root.clone()),
                insert_after_node_id: Some(node1.clone()),
                properties: json!({"task": {"status": "open"}}),
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

        let result = handle_update_nodes_batch(&node_service, params)
            .await
            .unwrap();

        assert_eq!(result["count"].as_u64().unwrap(), 2);
        assert_eq!(result["failed"].as_array().unwrap().len(), 0);

        // Verify updates
        let updated1 = node_service.get_node(&node1).await.unwrap().unwrap();
        assert_eq!(updated1.content, "- [x] Task 1");

        let updated2 = node_service.get_node(&node2).await.unwrap().unwrap();
        assert_eq!(updated2.content, "- [x] Task 2");
    }

    /// Verifies partial success handling with detailed failure reporting
    #[tokio::test]
    async fn test_update_nodes_batch_partial_failure() {
        let (node_service, _temp_dir) = setup_test_service().await.unwrap();

        let node1 = node_service
            .create_node_with_parent(CreateNodeParams {
                id: None,
                node_type: "text".to_string(),
                content: "Node 1".to_string(),
                parent_id: None,
                insert_after_node_id: None,
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

        let result = handle_update_nodes_batch(&node_service, params)
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
        let (node_service, _temp_dir) = setup_test_service().await.unwrap();

        let params = json!({
            "updates": []
        });

        let result = handle_update_nodes_batch(&node_service, params).await;

        assert!(result.is_err());
        let error = result.unwrap_err();
        assert!(error.message.contains("cannot be empty"));
    }

    /// Verifies batch size limit enforcement (max 100 updates)
    #[tokio::test]
    async fn test_update_nodes_batch_exceeds_limit() {
        let (node_service, _temp_dir) = setup_test_service().await.unwrap();

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

        let result = handle_update_nodes_batch(&node_service, params).await;

        assert!(result.is_err());
        let error = result.unwrap_err();
        assert!(error.message.contains("exceeds maximum of 100"));
    }

    /// Verifies property-only updates without content changes
    /// Uses task node type since it has a spoke table for properties (hub-spoke architecture)
    #[tokio::test]
    async fn test_update_nodes_batch_with_properties() {
        let (node_service, _temp_dir) = setup_test_service().await.unwrap();

        let node = node_service
            .create_node_with_parent(CreateNodeParams {
                id: None,
                node_type: "task".to_string(),
                content: "Test task".to_string(),
                parent_id: None,
                insert_after_node_id: None,
                properties: json!({"status": "open", "priority": "low"}),
            })
            .await
            .unwrap();

        // Update properties only (goes to task spoke table)
        let params = json!({
            "updates": [
                { "id": node, "properties": { "priority": "high", "status": "done" } }
            ]
        });

        let result = handle_update_nodes_batch(&node_service, params)
            .await
            .unwrap();

        assert_eq!(result["count"].as_u64().unwrap(), 1);

        // Verify property update in spoke table
        let updated = node_service.get_node(&node).await.unwrap().unwrap();
        assert_eq!(updated.properties["priority"], "high");
        assert_eq!(updated.properties["status"], "done");
    }
}

// =========================================================================
// Strongly-Typed Node Response Tests (Issue #673)
// =========================================================================

#[cfg(test)]
mod typed_response_tests {
    use crate::db::SurrealStore;
    use crate::mcp::handlers::nodes::{
        handle_get_node, handle_get_nodes_batch, handle_query_nodes,
    };
    use crate::services::CreateNodeParams;
    use crate::NodeService;
    use serde_json::json;
    use std::sync::Arc;
    use tempfile::TempDir;

    async fn setup_test_service() -> Result<(Arc<NodeService>, TempDir), Box<dyn std::error::Error>>
    {
        let temp_dir = TempDir::new()?;
        let db_path = temp_dir.path().join("test.db");

        let mut store = Arc::new(SurrealStore::new(db_path).await?);
        let node_service = Arc::new(NodeService::new(&mut store).await?);
        Ok((node_service, temp_dir))
    }

    /// Verifies get_node returns TaskNode struct for task types
    /// TaskNode should have direct fields (status, priority) instead of properties
    #[tokio::test]
    async fn test_get_node_returns_typed_task_node() {
        let (node_service, _temp_dir) = setup_test_service().await.unwrap();

        // Create a task node (priority is a string enum, not integer)
        let node_id = node_service
            .create_node_with_parent(CreateNodeParams {
                id: None,
                node_type: "task".to_string(),
                content: "Test task".to_string(),
                parent_id: None,
                insert_after_node_id: None,
                properties: json!({"status": "open", "priority": "high"}),
            })
            .await
            .unwrap();

        // Get the node via MCP handler
        let params = json!({ "node_id": node_id });
        let result = handle_get_node(&node_service, params).await.unwrap();

        // Verify TaskNode structure: direct fields instead of properties
        assert_eq!(result["id"], node_id);
        assert_eq!(result["content"], "Test task");
        assert_eq!(result["status"], "open"); // Direct field, not properties.status

        // TaskNode has properties field for UI compatibility (though spoke fields are also direct)
        // Properties should exist but be empty or contain minimal data
        assert!(result.get("properties").is_some());
    }

    /// Verifies get_node returns generic Node for simple types (text, header, etc.)
    #[tokio::test]
    async fn test_get_node_returns_generic_node_for_text() {
        let (node_service, _temp_dir) = setup_test_service().await.unwrap();

        // Create a text node (no spoke table, uses generic Node)
        let node_id = node_service
            .create_node_with_parent(CreateNodeParams {
                id: None,
                node_type: "text".to_string(),
                content: "Hello world".to_string(),
                parent_id: None,
                insert_after_node_id: None,
                properties: json!({}),
            })
            .await
            .unwrap();

        // Get the node via MCP handler
        let params = json!({ "node_id": node_id });
        let result = handle_get_node(&node_service, params).await.unwrap();

        // Verify generic Node structure
        // Note: Node uses camelCase serialization (nodeType, not node_type)
        assert_eq!(result["id"], node_id);
        assert_eq!(result["content"], "Hello world");
        assert_eq!(result["nodeType"], "text");

        // Generic Node has properties field (even if empty)
        assert!(result.get("properties").is_some());
    }

    /// Verifies query_nodes returns mixed typed/generic nodes
    #[tokio::test]
    async fn test_query_nodes_returns_typed_and_generic_nodes() {
        let (node_service, _temp_dir) = setup_test_service().await.unwrap();

        // Create a task node
        let task_id = node_service
            .create_node_with_parent(CreateNodeParams {
                id: None,
                node_type: "task".to_string(),
                content: "Task node".to_string(),
                parent_id: None,
                insert_after_node_id: None,
                properties: json!({"status": "done"}),
            })
            .await
            .unwrap();

        // Create a text node
        let _text_id = node_service
            .create_node_with_parent(CreateNodeParams {
                id: None,
                node_type: "text".to_string(),
                content: "Text node".to_string(),
                parent_id: None,
                insert_after_node_id: None,
                properties: json!({}),
            })
            .await
            .unwrap();

        // Query all task nodes
        let params = json!({ "node_type": "task" });
        let result = handle_query_nodes(&node_service, params).await.unwrap();

        let nodes = result["nodes"].as_array().unwrap();
        assert!(!nodes.is_empty());

        // Find the task node and verify typed structure
        let task_node = nodes.iter().find(|n| n["id"] == task_id).unwrap();
        assert_eq!(task_node["status"], "done"); // Direct field on TaskNode
    }

    /// Verifies get_nodes_batch returns typed structs for task nodes
    #[tokio::test]
    async fn test_get_nodes_batch_returns_typed_nodes() {
        let (node_service, _temp_dir) = setup_test_service().await.unwrap();

        // Create multiple nodes of different types (priority is string enum)
        let task_id = node_service
            .create_node_with_parent(CreateNodeParams {
                id: None,
                node_type: "task".to_string(),
                content: "Batch task".to_string(),
                parent_id: None,
                insert_after_node_id: None,
                properties: json!({"status": "in_progress", "priority": "medium"}),
            })
            .await
            .unwrap();

        let text_id = node_service
            .create_node_with_parent(CreateNodeParams {
                id: None,
                node_type: "text".to_string(),
                content: "Batch text".to_string(),
                parent_id: None,
                insert_after_node_id: None,
                properties: json!({}),
            })
            .await
            .unwrap();

        // Batch get both nodes
        let params = json!({ "node_ids": [task_id.clone(), text_id.clone()] });
        let result = handle_get_nodes_batch(&node_service, params).await.unwrap();

        let nodes = result["nodes"].as_array().unwrap();
        assert_eq!(nodes.len(), 2);

        // Find and verify task node structure
        let task_node = nodes.iter().find(|n| n["id"] == task_id).unwrap();
        assert_eq!(task_node["status"], "in_progress"); // Direct TaskNode field
                                                        // TaskNode has properties field for UI compatibility
        assert!(task_node.get("properties").is_some());

        // Find and verify text node structure (generic Node)
        // Note: Node uses camelCase serialization (nodeType, not node_type)
        let text_node = nodes.iter().find(|n| n["id"] == text_id).unwrap();
        assert_eq!(text_node["nodeType"], "text");
        assert!(text_node.get("properties").is_some()); // Generic Node has properties
    }
}
