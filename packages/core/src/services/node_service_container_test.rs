//! Comprehensive Integration Tests for Container Node Detection and Stale Marking
//!
//! These tests validate the critical bug fixes for Issue #107:
//! - Container nodes are correctly identified by `container_node_id IS NULL`
//! - New container nodes are automatically marked as stale for embedding generation
//! - Child node content updates mark parent containers as stale
//! - Node moves between containers mark both old and new containers as stale
//!
//! Tests prevent regression of the critical bug where the embedding system
//! incorrectly used `node_type == 'topic'` instead of `container_node_id IS NULL`.

#[cfg(test)]
mod tests {
    use crate::db::DatabaseService;
    use crate::models::{Node, NodeUpdate};
    use crate::services::NodeService;
    use serde_json::json;
    use std::sync::Arc;
    use tempfile::TempDir;

    /// Helper to create test services
    /// Returns (node_service, db_service, _temp_dir) - temp_dir must be kept alive for test duration
    async fn create_test_services() -> (Arc<NodeService>, Arc<DatabaseService>, TempDir) {
        let temp_dir = TempDir::new().unwrap();
        let db_path = temp_dir.path().join("test.db");

        let db_service = DatabaseService::new(db_path).await.unwrap();
        let node_service = Arc::new(NodeService::new(db_service.clone()).unwrap());
        let db_service = Arc::new(db_service);

        (node_service, db_service, temp_dir)
    }

    /// Helper to check if a node is marked as stale in the database
    async fn is_node_stale(db: &DatabaseService, node_id: &str) -> Result<bool, Box<dyn std::error::Error>> {
        let conn = db.connect_with_timeout().await?;
        let mut stmt = conn
            .prepare("SELECT embedding_stale FROM nodes WHERE id = ?")
            .await?;
        let mut rows = stmt.query([node_id]).await?;

        if let Some(row) = rows.next().await? {
            let stale: bool = row.get(0)?;
            Ok(stale)
        } else {
            Err("Node not found".into())
        }
    }

    /// Helper to mark a node as not stale (simulating successful embedding)
    async fn mark_not_stale(db: &DatabaseService, node_id: &str) -> Result<(), Box<dyn std::error::Error>> {
        let conn = db.connect_with_timeout().await?;
        conn.execute(
            "UPDATE nodes SET embedding_stale = FALSE WHERE id = ?",
            [node_id],
        )
        .await?;
        Ok(())
    }

    #[tokio::test]
    async fn test_container_node_created_as_stale() {
        let (service, db, _temp) = create_test_services().await;

        // Create a container node (no container_node_id = root level)
        let container_node = Node::new("text".to_string(), "Container content".to_string(), None, json!({}));
        let container_id = container_node.id.clone();

        service.create_node(container_node).await.unwrap();

        // Verify container node is marked as stale
        let is_stale = is_node_stale(&db, &container_id).await.unwrap();
        assert!(is_stale, "Container node should be marked as stale on creation");
    }

    #[tokio::test]
    async fn test_child_node_not_created_as_stale() {
        let (service, db, _temp) = create_test_services().await;

        // Create a container node first
        let container_node = Node::new("text".to_string(), "Container".to_string(), None, json!({}));
        let container_id = container_node.id.clone();
        service.create_node(container_node).await.unwrap();

        // Create a child node inside the container
        let mut child_node = Node::new(
            "text".to_string(),
            "Child content".to_string(),
            None,
            json!({}),
        );
        child_node.container_node_id = Some(container_id.clone());
        let child_id = child_node.id.clone();

        service.create_node(child_node).await.unwrap();

        // Verify child node is NOT marked as stale (only containers need embeddings)
        let is_stale = is_node_stale(&db, &child_id).await.unwrap();
        assert!(!is_stale, "Child node should NOT be marked as stale on creation");
    }

    #[tokio::test]
    async fn test_container_content_update_marks_stale() {
        let (service, db, _temp) = create_test_services().await;

        // Create container and mark it as not stale (simulate existing embedding)
        let container_node = Node::new("text".to_string(), "Original content".to_string(), None, json!({}));
        let container_id = container_node.id.clone();
        service.create_node(container_node).await.unwrap();
        mark_not_stale(&db, &container_id).await.unwrap();

        // Verify it's not stale before update
        assert!(!is_node_stale(&db, &container_id).await.unwrap());

        // Update container content
        let update = NodeUpdate::new().with_content("Updated content".to_string());
        service.update_node(&container_id, update).await.unwrap();

        // Verify container is now marked as stale
        let is_stale = is_node_stale(&db, &container_id).await.unwrap();
        assert!(is_stale, "Container should be marked as stale after content update");
    }

    #[tokio::test]
    async fn test_child_content_update_marks_parent_stale() {
        let (service, db, _temp) = create_test_services().await;

        // Create container
        let container_node = Node::new("text".to_string(), "Container".to_string(), None, json!({}));
        let container_id = container_node.id.clone();
        service.create_node(container_node).await.unwrap();
        mark_not_stale(&db, &container_id).await.unwrap();

        // Create child inside container
        let mut child_node = Node::new(
            "text".to_string(),
            "Child content".to_string(),
            None,
            json!({}),
        );
        child_node.container_node_id = Some(container_id.clone());
        let child_id = child_node.id.clone();
        service.create_node(child_node).await.unwrap();

        // Verify container is not stale before child update
        assert!(!is_node_stale(&db, &container_id).await.unwrap());

        // Update child content
        let update = NodeUpdate::new().with_content("Updated child content".to_string());
        service.update_node(&child_id, update).await.unwrap();

        // Verify parent container is now marked as stale
        let is_stale = is_node_stale(&db, &container_id).await.unwrap();
        assert!(
            is_stale,
            "Parent container should be marked as stale when child content changes"
        );
    }

    #[tokio::test]
    async fn test_node_move_marks_both_containers_stale() {
        let (service, db, _temp) = create_test_services().await;

        // Create two container nodes
        let container1 = Node::new("text".to_string(), "Container 1".to_string(), None, json!({}));
        let container1_id = container1.id.clone();
        service.create_node(container1).await.unwrap();
        mark_not_stale(&db, &container1_id).await.unwrap();

        let container2 = Node::new("text".to_string(), "Container 2".to_string(), None, json!({}));
        let container2_id = container2.id.clone();
        service.create_node(container2).await.unwrap();
        mark_not_stale(&db, &container2_id).await.unwrap();

        // Create a child node in container1
        let mut child_node = Node::new(
            "text".to_string(),
            "Child content".to_string(),
            None,
            json!({}),
        );
        child_node.container_node_id = Some(container1_id.clone());
        let child_id = child_node.id.clone();
        service.create_node(child_node).await.unwrap();

        // Verify both containers are not stale before move
        assert!(!is_node_stale(&db, &container1_id).await.unwrap());
        assert!(!is_node_stale(&db, &container2_id).await.unwrap());

        // Move child from container1 to container2
        let mut update = NodeUpdate::new();
        update.container_node_id = Some(Some(container2_id.clone()));
        service.update_node(&child_id, update).await.unwrap();

        // Verify BOTH old and new containers are marked as stale
        let container1_stale = is_node_stale(&db, &container1_id).await.unwrap();
        let container2_stale = is_node_stale(&db, &container2_id).await.unwrap();

        assert!(
            container1_stale,
            "Old container should be marked as stale when node moves out"
        );
        assert!(
            container2_stale,
            "New container should be marked as stale when node moves in"
        );
    }

    #[tokio::test]
    async fn test_child_non_content_update_does_not_mark_parent_stale() {
        let (service, db, _temp) = create_test_services().await;

        // Create container
        let container_node = Node::new("text".to_string(), "Container".to_string(), None, json!({}));
        let container_id = container_node.id.clone();
        service.create_node(container_node).await.unwrap();
        mark_not_stale(&db, &container_id).await.unwrap();

        // Create child inside container
        let mut child_node = Node::new(
            "text".to_string(),
            "Child content".to_string(),
            None,
            json!({}),
        );
        child_node.container_node_id = Some(container_id.clone());
        let child_id = child_node.id.clone();
        service.create_node(child_node).await.unwrap();

        // Verify container is not stale
        assert!(!is_node_stale(&db, &container_id).await.unwrap());

        // Update child properties (NOT content)
        let update = NodeUpdate::new().with_properties(json!({"key": "value"}));
        service.update_node(&child_id, update).await.unwrap();

        // Verify parent container is still NOT stale (only content changes matter)
        let is_stale = is_node_stale(&db, &container_id).await.unwrap();
        assert!(
            !is_stale,
            "Parent container should NOT be marked as stale for non-content child updates"
        );
    }

    #[tokio::test]
    async fn test_container_detection_is_null_based_not_type_based() {
        let (service, db, _temp) = create_test_services().await;

        // Create a "task" node at root level (container_node_id = None)
        // This tests that container detection uses NULL check, not node_type == 'topic'
        let task_container = Node::new("task".to_string(), "Task as container".to_string(), None, json!({}));
        let task_id = task_container.id.clone();
        service.create_node(task_container).await.unwrap();

        // Verify task node is marked as stale (because it's a container)
        let is_stale = is_node_stale(&db, &task_id).await.unwrap();
        assert!(
            is_stale,
            "ANY node type at root level (container_node_id IS NULL) should be marked as stale"
        );

        // Create a "text" node inside another container
        let container = Node::new("text".to_string(), "Container".to_string(), None, json!({}));
        let container_id = container.id.clone();
        service.create_node(container).await.unwrap();

        let mut child = Node::new("text".to_string(), "Child".to_string(), None, json!({}));
        child.container_node_id = Some(container_id);
        let child_id = child.id.clone();
        service.create_node(child).await.unwrap();

        // Verify child is NOT marked as stale (even though it's type "text")
        let is_stale = is_node_stale(&db, &child_id).await.unwrap();
        assert!(
            !is_stale,
            "Node with container_node_id should NOT be marked as stale, regardless of type"
        );
    }

    #[tokio::test]
    async fn test_multiple_children_updates_marks_parent_once() {
        let (service, db, _temp) = create_test_services().await;

        // Create container
        let container_node = Node::new("text".to_string(), "Container".to_string(), None, json!({}));
        let container_id = container_node.id.clone();
        service.create_node(container_node).await.unwrap();
        mark_not_stale(&db, &container_id).await.unwrap();

        // Create multiple children
        let mut child1 = Node::new("text".to_string(), "Child 1".to_string(), None, json!({}));
        child1.container_node_id = Some(container_id.clone());
        let child1_id = child1.id.clone();
        service.create_node(child1).await.unwrap();

        let mut child2 = Node::new("text".to_string(), "Child 2".to_string(), None, json!({}));
        child2.container_node_id = Some(container_id.clone());
        let child2_id = child2.id.clone();
        service.create_node(child2).await.unwrap();

        // Update first child
        let update = NodeUpdate::new().with_content("Updated child 1".to_string());
        service.update_node(&child1_id, update).await.unwrap();

        // Verify container is stale
        assert!(is_node_stale(&db, &container_id).await.unwrap());

        // Mark container as not stale again
        mark_not_stale(&db, &container_id).await.unwrap();

        // Update second child
        let update = NodeUpdate::new().with_content("Updated child 2".to_string());
        service.update_node(&child2_id, update).await.unwrap();

        // Verify container is stale again
        assert!(
            is_node_stale(&db, &container_id).await.unwrap(),
            "Container should be marked stale for each child content update"
        );
    }
}
