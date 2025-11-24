//! Event Emission Tests (Issue #643)
//!
//! Tests that verify correct event emission for all major operations.
//! Ensures the event-driven architecture emits exactly one event per operation,
//! and that events are emitted AFTER the transaction completes successfully.

#[cfg(test)]
mod event_emission_tests {
    use anyhow::Result;
    use nodespace_core::db::{DomainEvent, SurrealStore};
    use nodespace_core::models::Node;
    use serde_json::json;
    use tempfile::TempDir;
    use tokio::time::{timeout, Duration};

    /// Helper to create test database
    async fn create_test_db() -> Result<(SurrealStore, TempDir)> {
        let temp_dir = TempDir::new()?;
        let db_path = temp_dir.path().join("test.db");
        let store = SurrealStore::new(db_path).await?;
        Ok((store, temp_dir))
    }

    /// Helper to create a test root node
    async fn create_root_node(store: &SurrealStore, node_type: &str) -> Result<Node> {
        let node = Node::new(
            node_type.to_string(),
            format!("Test {} node", node_type),
            json!({}),
        );

        store.create_node(node.clone()).await?;
        Ok(node)
    }

    #[tokio::test]
    async fn test_create_node_emits_node_created_event() -> Result<()> {
        let (store, _temp_dir) = create_test_db().await?;

        // Subscribe to events
        let mut rx = store.subscribe_to_events();

        // Create a node
        let node = Node::new("text".to_string(), "Test content".to_string(), json!({}));

        let expected_id = node.id.clone();
        store.create_node(node).await?;

        // Receive the emitted event
        let event = timeout(Duration::from_secs(1), rx.recv())
            .await
            .expect("Event should be emitted within 1 second")
            .expect("Should receive event");

        // Verify it's a NodeCreated event
        match event {
            DomainEvent::NodeCreated(created_node) => {
                assert_eq!(created_node.id, expected_id);
                assert_eq!(created_node.node_type, "text");
                assert_eq!(created_node.content, "Test content");
            }
            _ => panic!("Expected NodeCreated event, got {:?}", event),
        }

        Ok(())
    }

    #[tokio::test]
    async fn test_update_node_emits_node_updated_event() -> Result<()> {
        let (store, _temp_dir) = create_test_db().await?;

        // Create a node first
        let node = create_root_node(&store, "text").await?;
        let node_id = node.id.clone();

        // Subscribe to events (AFTER creation to avoid catching NodeCreated)
        let mut rx = store.subscribe_to_events();

        // Update the node
        let _updated_node = store
            .update_node(
                &node_id,
                nodespace_core::models::NodeUpdate {
                    content: Some("Updated content".to_string()),
                    ..Default::default()
                },
            )
            .await?;

        // Receive the emitted event
        let event = timeout(Duration::from_secs(1), rx.recv())
            .await
            .expect("Event should be emitted within 1 second")
            .expect("Should receive event");

        // Verify it's a NodeUpdated event
        match event {
            DomainEvent::NodeUpdated(updated) => {
                assert_eq!(updated.id, node_id);
                assert_eq!(updated.content, "Updated content");
            }
            _ => panic!("Expected NodeUpdated event, got {:?}", event),
        }

        Ok(())
    }

    #[tokio::test]
    async fn test_delete_node_emits_node_deleted_event() -> Result<()> {
        let (store, _temp_dir) = create_test_db().await?;

        // Create a node first
        let node = create_root_node(&store, "text").await?;
        let node_id = node.id.clone();

        // Subscribe to events (AFTER creation to avoid catching NodeCreated)
        let mut rx = store.subscribe_to_events();

        // Delete the node
        let result = store.delete_node(&node_id).await?;
        assert!(result.existed);

        // Receive the emitted event
        let event = timeout(Duration::from_secs(1), rx.recv())
            .await
            .expect("Event should be emitted within 1 second")
            .expect("Should receive event");

        // Verify it's a NodeDeleted event
        match event {
            DomainEvent::NodeDeleted { id } => {
                assert_eq!(id, node_id);
            }
            _ => panic!("Expected NodeDeleted event, got {:?}", event),
        }

        Ok(())
    }

    #[tokio::test]
    async fn test_move_node_to_new_parent_emits_edge_updated_event() -> Result<()> {
        let (store, _temp_dir) = create_test_db().await?;

        // Create parent and child nodes
        let parent1 = create_root_node(&store, "text").await?;
        let parent2 = create_root_node(&store, "text").await?;
        let child = create_root_node(&store, "text").await?;

        // Create initial parent-child relationship
        store.move_node(&child.id, Some(&parent1.id), None).await?;

        // Subscribe to events (AFTER creation)
        let mut rx = store.subscribe_to_events();

        // Move child to new parent
        store.move_node(&child.id, Some(&parent2.id), None).await?;

        // Receive the emitted event
        let event = timeout(Duration::from_secs(1), rx.recv())
            .await
            .expect("Event should be emitted within 1 second")
            .expect("Should receive event");

        // Verify it's an EdgeUpdated event for hierarchy
        match event {
            DomainEvent::EdgeUpdated(edge) => match edge {
                nodespace_core::db::EdgeRelationship::Hierarchy(rel) => {
                    assert_eq!(rel.parent_id, parent2.id);
                    assert_eq!(rel.child_id, child.id);
                }
                _ => panic!("Expected hierarchy edge, got {:?}", edge),
            },
            _ => panic!("Expected EdgeUpdated event, got {:?}", event),
        }

        Ok(())
    }

    #[tokio::test]
    async fn test_move_node_to_root_emits_edge_deleted_event() -> Result<()> {
        let (store, _temp_dir) = create_test_db().await?;

        // Create parent and child nodes
        let parent = create_root_node(&store, "text").await?;
        let child = create_root_node(&store, "text").await?;

        // Create parent-child relationship
        store.move_node(&child.id, Some(&parent.id), None).await?;

        // Subscribe to events (AFTER creation)
        let mut rx = store.subscribe_to_events();

        // Move child to root (delete parent edge)
        store.move_node(&child.id, None, None).await?;

        // Receive the emitted event
        let event = timeout(Duration::from_secs(1), rx.recv())
            .await
            .expect("Event should be emitted within 1 second")
            .expect("Should receive event");

        // Verify it's an EdgeDeleted event
        match event {
            DomainEvent::EdgeDeleted { id } => {
                // Event ID should indicate has_child edge for this node
                assert!(id.contains(&child.id));
            }
            _ => panic!("Expected EdgeDeleted event, got {:?}", event),
        }

        Ok(())
    }

    #[tokio::test]
    async fn test_create_child_node_atomic_emits_both_node_and_edge_events() -> Result<()> {
        let (store, _temp_dir) = create_test_db().await?;

        // Create parent node
        let parent = create_root_node(&store, "text").await?;

        // Subscribe to events
        let mut rx = store.subscribe_to_events();

        // Create child node atomically with parent edge
        let child = store
            .create_child_node_atomic(&parent.id, "text", "Child content", json!({}))
            .await?;

        // Should receive TWO events: NodeCreated and EdgeCreated
        // Event 1: NodeCreated
        let event1 = timeout(Duration::from_secs(1), rx.recv())
            .await
            .expect("First event should be NodeCreated")
            .expect("Should receive first event");

        match event1 {
            DomainEvent::NodeCreated(node) => {
                assert_eq!(node.id, child.id);
                assert_eq!(node.content, "Child content");
            }
            _ => panic!("Expected first event to be NodeCreated, got {:?}", event1),
        }

        // Event 2: EdgeCreated
        let event2 = timeout(Duration::from_secs(1), rx.recv())
            .await
            .expect("Second event should be EdgeCreated")
            .expect("Should receive second event");

        match event2 {
            DomainEvent::EdgeCreated(edge) => match edge {
                nodespace_core::db::EdgeRelationship::Hierarchy(rel) => {
                    assert_eq!(rel.parent_id, parent.id);
                    assert_eq!(rel.child_id, child.id);
                }
                _ => panic!("Expected hierarchy edge, got {:?}", edge),
            },
            _ => panic!("Expected second event to be EdgeCreated, got {:?}", event2),
        }

        Ok(())
    }

    #[tokio::test]
    async fn test_delete_node_cascade_atomic_emits_events() -> Result<()> {
        let (store, _temp_dir) = create_test_db().await?;

        // Create parent and child nodes
        let parent = create_root_node(&store, "text").await?;
        let child = store
            .create_child_node_atomic(&parent.id, "text", "Child content", json!({}))
            .await?;

        // Subscribe to events (AFTER creation)
        let mut rx = store.subscribe_to_events();

        // Delete child node (cascade)
        let result = store.delete_node_cascade_atomic(&child.id).await?;
        assert!(result.existed);

        // Should receive NodeDeleted event for the child
        let event = timeout(Duration::from_secs(1), rx.recv())
            .await
            .expect("Event should be emitted within 1 second")
            .expect("Should receive event");

        match event {
            DomainEvent::NodeDeleted { id } => {
                assert_eq!(id, child.id);
            }
            _ => panic!("Expected NodeDeleted event, got {:?}", event),
        }

        Ok(())
    }

    #[tokio::test]
    async fn test_only_one_event_emitted_per_operation() -> Result<()> {
        let (store, _temp_dir) = create_test_db().await?;

        // Create a node
        let node = create_root_node(&store, "text").await?;
        let node_id = node.id.clone();

        // Subscribe to events
        let mut rx = store.subscribe_to_events();

        // Update the node
        store
            .update_node(
                &node_id,
                nodespace_core::models::NodeUpdate {
                    content: Some("Updated".to_string()),
                    ..Default::default()
                },
            )
            .await?;

        // Should receive exactly ONE event
        let event1 = timeout(Duration::from_secs(1), rx.recv())
            .await
            .expect("Should receive event")
            .expect("Should receive event");

        assert!(matches!(event1, DomainEvent::NodeUpdated(_)));

        // Attempting to receive another event should timeout
        let result = timeout(Duration::from_millis(100), rx.recv()).await;
        assert!(
            result.is_err(),
            "Should not receive a second event for single update"
        );

        Ok(())
    }
}
