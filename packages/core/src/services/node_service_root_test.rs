//! Comprehensive Integration Tests for Root Node Detection and Stale Marking
//!
//! **STATUS: TEMPORARILY DISABLED** (Issue #481)
//!
//! These tests validate embedding staleness tracking which is temporarily disabled
//! during the SurrealDB migration. Tests will be re-enabled after embedding service
//! is migrated to SurrealStore.
//!
//! Original tests validated Issue #107 fixes:
//! - Root nodes are correctly identified by `root_id IS NULL`
//! - New root nodes are automatically marked as stale for embedding generation
//! - Child node content updates mark parent roots as stale
//! - Node moves between roots mark both old and new roots as stale

#[cfg(test)]
#[cfg_attr(test, allow(dead_code, unused_imports))]
mod disabled_embedding_tests {
    // These tests are temporarily disabled - see issue #481
    use crate::db::SurrealStore;
    use crate::models::{Node, NodeUpdate};
    use crate::services::NodeService;
    use serde_json::json;
    use std::sync::Arc;
    use tempfile::TempDir;

    /// Helper to create test services
    /// Returns (node_service, store, _temp_dir) - temp_dir must be kept alive for test duration
    async fn create_test_services() -> (Arc<NodeService>, Arc<SurrealStore>, TempDir) {
        let temp_dir = TempDir::new().unwrap();
        let db_path = temp_dir.path().join("test.db");

        let store = Arc::new(SurrealStore::new(db_path).await.unwrap());
        let node_service = Arc::new(NodeService::new(store.clone()).unwrap());

        (node_service, store, temp_dir)
    }

    /// Helper to check if a node is marked as stale in the database
    /// TODO: Re-implement with SurrealDB queries when embedding tests are re-enabled (Issue #481)
    #[allow(dead_code)]
    fn is_node_stale(
        _store: &SurrealStore,
        _node_id: &str,
    ) -> Result<bool, Box<dyn std::error::Error>> {
        // Disabled during SurrealDB migration
        unimplemented!("Embedding staleness tests temporarily disabled - see Issue #481")
    }

    /// Helper to mark a node as not stale (simulating successful embedding)
    /// TODO: Re-implement with SurrealDB queries when embedding tests are re-enabled (Issue #481)
    #[allow(dead_code)]
    fn mark_not_stale(
        _store: &SurrealStore,
        _node_id: &str,
    ) -> Result<(), Box<dyn std::error::Error>> {
        // Disabled during SurrealDB migration
        unimplemented!("Embedding staleness tests temporarily disabled - see Issue #481")
    }

    #[tokio::test]
    #[ignore = "Temporarily disabled during SurrealDB migration - see Issue #481"]
    async fn test_root_node_created_as_stale() {
        let (service, store, _temp) = create_test_services().await;

        // Create a root node (root level - no parent edges)
        let root_node = Node::new("text".to_string(), "Root content".to_string(), json!({}));
        let root_id = root_node.id.clone();

        service.create_node(root_node).await.unwrap();

        // Verify root node is marked as stale
        let is_stale = is_node_stale(&store, &root_id).unwrap();
        assert!(is_stale, "Root node should be marked as stale on creation");
    }

    #[tokio::test]
    #[ignore = "Temporarily disabled during SurrealDB migration - see Issue #481"]
    async fn test_child_node_not_created_as_stale() {
        let (service, store, _temp) = create_test_services().await;

        // Create a root node first
        let root_node = Node::new("text".to_string(), "Root".to_string(), json!({}));
        let _root_id = root_node.id.clone();
        service.create_node(root_node).await.unwrap();

        // Create a child node inside the root (using parent relationship via edges)
        let child_node = Node::new("text".to_string(), "Child content".to_string(), json!({}));
        let child_id = child_node.id.clone();

        service.create_node(child_node).await.unwrap();

        // Verify child node is NOT marked as stale (only roots need embeddings)
        let is_stale = is_node_stale(&store, &child_id).unwrap();
        assert!(
            !is_stale,
            "Child node should NOT be marked as stale on creation"
        );
    }

    #[tokio::test]
    #[ignore = "Temporarily disabled during SurrealDB migration - see Issue #481"]
    async fn test_root_content_update_marks_stale() {
        let (service, store, _temp) = create_test_services().await;

        // Create root and mark it as not stale (simulate existing embedding)
        let root_node = Node::new(
            "text".to_string(),
            "Original content".to_string(),
            json!({}),
        );
        let root_id = root_node.id.clone();
        service.create_node(root_node).await.unwrap();
        mark_not_stale(&store, &root_id).unwrap();

        // Verify it's not stale before update
        assert!(!is_node_stale(&store, &root_id).unwrap());

        // Update root content
        let update = NodeUpdate::new().with_content("Updated content".to_string());
        service.update_node(&root_id, update).await.unwrap();

        // Verify root is now marked as stale
        let is_stale = is_node_stale(&store, &root_id).unwrap();
        assert!(
            is_stale,
            "Root should be marked as stale after content update"
        );
    }

    #[tokio::test]
    #[ignore = "Temporarily disabled during SurrealDB migration - see Issue #481"]
    async fn test_child_content_update_marks_parent_stale() {
        let (service, store, _temp) = create_test_services().await;

        // Create root
        let root_node = Node::new("text".to_string(), "Root".to_string(), json!({}));
        let root_id = root_node.id.clone();
        service.create_node(root_node).await.unwrap();
        mark_not_stale(&store, &root_id).unwrap();

        // Create child inside root (using parent relationship via edges)
        let child_node = Node::new("text".to_string(), "Child content".to_string(), json!({}));
        let child_id = child_node.id.clone();
        service.create_node(child_node).await.unwrap();

        // Verify root is not stale before child update
        assert!(!is_node_stale(&store, &root_id).unwrap());

        // Update child content
        let update = NodeUpdate::new().with_content("Updated child content".to_string());
        service.update_node(&child_id, update).await.unwrap();

        // Verify parent root is now marked as stale
        let is_stale = is_node_stale(&store, &root_id).unwrap();
        assert!(
            is_stale,
            "Parent root should be marked as stale when child content changes"
        );
    }

    #[tokio::test]
    #[ignore = "Temporarily disabled during SurrealDB migration - see Issue #481"]
    async fn test_node_move_marks_both_roots_stale() {
        let (service, store, _temp) = create_test_services().await;

        // Create two root nodes
        let root1 = Node::new("text".to_string(), "Root 1".to_string(), json!({}));
        let root1_id = root1.id.clone();
        service.create_node(root1).await.unwrap();
        mark_not_stale(&store, &root1_id).unwrap();

        let root2 = Node::new("text".to_string(), "Root 2".to_string(), json!({}));
        let root2_id = root2.id.clone();
        service.create_node(root2).await.unwrap();
        mark_not_stale(&store, &root2_id).unwrap();

        // Create a child node in root1 (using parent relationship via edges)
        let child_node = Node::new("text".to_string(), "Child content".to_string(), json!({}));
        let child_id = child_node.id.clone();
        service.create_node(child_node).await.unwrap();

        // Verify both roots are not stale before move
        assert!(!is_node_stale(&store, &root1_id).unwrap());
        assert!(!is_node_stale(&store, &root2_id).unwrap());

        // Move child from root1 to root2 (using move_node API)
        service
            .move_node(&child_id, Some(&root2_id), None)
            .await
            .unwrap();

        // Verify BOTH old and new roots are marked as stale
        let root1_stale = is_node_stale(&store, &root1_id).unwrap();
        let root2_stale = is_node_stale(&store, &root2_id).unwrap();

        assert!(
            root1_stale,
            "Old root should be marked as stale when node moves out"
        );
        assert!(
            root2_stale,
            "New root should be marked as stale when node moves in"
        );
    }

    #[tokio::test]
    #[ignore = "Temporarily disabled during SurrealDB migration - see Issue #481"]
    async fn test_child_non_content_update_does_not_mark_parent_stale() {
        let (service, store, _temp) = create_test_services().await;

        // Create root
        let root_node = Node::new("text".to_string(), "Root".to_string(), json!({}));
        let root_id = root_node.id.clone();
        service.create_node(root_node).await.unwrap();
        mark_not_stale(&store, &root_id).unwrap();

        // Create child inside root (using parent relationship)
        let child_node = Node::new("text".to_string(), "Child content".to_string(), json!({}));
        let child_id = child_node.id.clone();
        service.create_node(child_node).await.unwrap();

        // Verify root is not stale
        assert!(!is_node_stale(&store, &root_id).unwrap());

        // Update child properties (NOT content)
        let update = NodeUpdate::new().with_properties(json!({"key": "value"}));
        service.update_node(&child_id, update).await.unwrap();

        // Verify parent root is still NOT stale (only content changes matter)
        let is_stale = is_node_stale(&store, &root_id).unwrap();
        assert!(
            !is_stale,
            "Parent root should NOT be marked as stale for non-content child updates"
        );
    }

    #[tokio::test]
    #[ignore = "Temporarily disabled during SurrealDB migration - see Issue #481"]
    async fn test_root_detection_is_null_based_not_type_based() {
        let (service, store, _temp) = create_test_services().await;

        // Create a "task" node at root level (root_id = None)
        // This tests that root detection uses NULL check, not node_type == 'topic'
        let task_root = Node::new(
            "task".to_string(),
            "Task as root".to_string(),
            json!({"status": "OPEN"}),
        );
        let task_id = task_root.id.clone();
        service.create_node(task_root).await.unwrap();

        // Verify task node is marked as stale (because it's a root)
        let is_stale = is_node_stale(&store, &task_id).unwrap();
        assert!(
            is_stale,
            "ANY node type at root level (root_id IS NULL) should be marked as stale"
        );

        // Create a "text" node inside another root (using parent relationship)
        let root = Node::new("text".to_string(), "Root".to_string(), json!({}));
        let _root_id = root.id.clone();
        service.create_node(root).await.unwrap();

        let child = Node::new("text".to_string(), "Child".to_string(), json!({}));
        let child_id = child.id.clone();
        service.create_node(child).await.unwrap();

        // Verify child is NOT marked as stale (even though it's type "text")
        let is_stale = is_node_stale(&store, &child_id).unwrap();
        assert!(
            !is_stale,
            "Node with root_id should NOT be marked as stale, regardless of type"
        );
    }

    #[tokio::test]
    #[ignore = "Temporarily disabled during SurrealDB migration - see Issue #481"]
    async fn test_multiple_children_updates_marks_parent_once() {
        let (service, store, _temp) = create_test_services().await;

        // Create root
        let root_node = Node::new("text".to_string(), "Root".to_string(), json!({}));
        let root_id = root_node.id.clone();
        service.create_node(root_node).await.unwrap();
        mark_not_stale(&store, &root_id).unwrap();

        // Create multiple children (using parent relationship)
        let child1 = Node::new("text".to_string(), "Child 1".to_string(), json!({}));
        let child1_id = child1.id.clone();
        service.create_node(child1).await.unwrap();

        let child2 = Node::new("text".to_string(), "Child 2".to_string(), json!({}));
        let child2_id = child2.id.clone();
        service.create_node(child2).await.unwrap();

        // Update first child
        let update = NodeUpdate::new().with_content("Updated child 1".to_string());
        service.update_node(&child1_id, update).await.unwrap();

        // Verify root is stale
        assert!(is_node_stale(&store, &root_id).unwrap());

        // Mark root as not stale again
        mark_not_stale(&store, &root_id).unwrap();

        // Update second child
        let update = NodeUpdate::new().with_content("Updated child 2".to_string());
        service.update_node(&child2_id, update).await.unwrap();

        // Verify root is stale again
        assert!(
            is_node_stale(&store, &root_id).unwrap(),
            "Root should be marked stale for each child content update"
        );
    }
}
