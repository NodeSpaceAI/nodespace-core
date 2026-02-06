//! Event Emission Tests (Issue #643, updated for Issue #665)
//!
//! Tests that verify correct event emission for all major operations.
//! As of Issue #665, events are now emitted at the NodeService layer
//! (not SurrealStore) to support client filtering.
//!
//! These tests verify:
//! 1. Correct events are emitted for each operation type
//! 2. Events contain proper source_client_id when set via with_client()
//! 3. Events are emitted AFTER the transaction completes successfully

#[cfg(test)]
mod event_emission_tests {
    use anyhow::Result;
    use nodespace_core::db::{DomainEvent, SurrealStore};
    use nodespace_core::models::Node;
    use nodespace_core::services::NodeService;
    use serde_json::json;
    use std::sync::Arc;
    use tempfile::TempDir;
    use tokio::time::{timeout, Duration};

    const TEST_CLIENT_ID: &str = "test-client";

    /// Helper to create test database and NodeService
    async fn create_test_service() -> Result<(NodeService, TempDir)> {
        let temp_dir = TempDir::new()?;
        let db_path = temp_dir.path().join("test.db");
        let mut store = Arc::new(SurrealStore::new(db_path).await?);
        let service = NodeService::new(&mut store).await?;
        Ok((service, temp_dir))
    }

    /// Helper to create a test root node via NodeService
    async fn create_root_node(service: &NodeService, node_type: &str) -> Result<Node> {
        let node = Node::new(
            node_type.to_string(),
            format!("Test {} node", node_type),
            json!({}),
        );

        service
            .with_client(TEST_CLIENT_ID)
            .create_node(node.clone())
            .await?;

        // Fetch back to get database-generated timestamps
        let created = service
            .get_node(&node.id)
            .await?
            .expect("Node should exist");
        Ok(created)
    }

    #[tokio::test]
    async fn test_create_node_emits_node_created_event() -> Result<()> {
        let (service, _temp_dir) = create_test_service().await?;

        // Subscribe to events
        let mut rx = service.subscribe_to_events();

        // Create a node
        let node = Node::new("text".to_string(), "Test content".to_string(), json!({}));

        let expected_id = node.id.clone();
        service
            .with_client(TEST_CLIENT_ID)
            .create_node(node)
            .await?;

        // Receive the emitted event
        let event = timeout(Duration::from_secs(1), rx.recv())
            .await
            .expect("Event should be emitted within 1 second")
            .expect("Should receive event");

        // Verify it's a NodeCreated event with correct client_id (Issue #724: ID-only events)
        // Issue #832: node_type is now included for reactive UI updates
        match event {
            DomainEvent::NodeCreated {
                node_id,
                node_type,
                source_client_id,
            } => {
                assert_eq!(node_id, expected_id);
                assert_eq!(node_type, "text");
                assert_eq!(source_client_id, Some(TEST_CLIENT_ID.to_string()));
            }
            _ => panic!("Expected NodeCreated event, got {:?}", event),
        }

        Ok(())
    }

    #[tokio::test]
    async fn test_update_node_emits_node_updated_event() -> Result<()> {
        let (service, _temp_dir) = create_test_service().await?;

        // Create a node first
        let node = create_root_node(&service, "text").await?;
        let node_id = node.id.clone();

        // Subscribe to events (AFTER creation to avoid catching NodeCreated)
        let mut rx = service.subscribe_to_events();

        // Update the node
        service
            .with_client(TEST_CLIENT_ID)
            .update_node_unchecked(
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

        // Verify it's a NodeUpdated event with correct client_id (Issue #724: ID-only events)
        match event {
            DomainEvent::NodeUpdated {
                node_id: updated_id,
                source_client_id,
            } => {
                assert_eq!(updated_id, node_id);
                assert_eq!(source_client_id, Some(TEST_CLIENT_ID.to_string()));
            }
            _ => panic!("Expected NodeUpdated event, got {:?}", event),
        }

        Ok(())
    }

    #[tokio::test]
    async fn test_delete_node_emits_node_deleted_event() -> Result<()> {
        let (service, _temp_dir) = create_test_service().await?;

        // Create a node first
        let node = create_root_node(&service, "text").await?;
        let node_id = node.id.clone();

        // Subscribe to events (AFTER creation to avoid catching NodeCreated)
        let mut rx = service.subscribe_to_events();

        // Delete the node
        let result = service
            .with_client(TEST_CLIENT_ID)
            .delete_node_unchecked(&node_id)
            .await?;
        assert!(result.existed);

        // Receive the emitted event
        let event = timeout(Duration::from_secs(1), rx.recv())
            .await
            .expect("Event should be emitted within 1 second")
            .expect("Should receive event");

        // Verify it's a NodeDeleted event with correct client_id
        match event {
            DomainEvent::NodeDeleted {
                id,
                source_client_id,
            } => {
                assert_eq!(id, node_id);
                assert_eq!(source_client_id, Some(TEST_CLIENT_ID.to_string()));
            }
            _ => panic!("Expected NodeDeleted event, got {:?}", event),
        }

        Ok(())
    }

    #[tokio::test]
    async fn test_move_node_to_new_parent_emits_relationship_updated_event() -> Result<()> {
        let (service, _temp_dir) = create_test_service().await?;

        // Create parent and child nodes
        let parent1 = create_root_node(&service, "text").await?;
        let parent2 = create_root_node(&service, "text").await?;
        let child = create_root_node(&service, "text").await?;

        // Create initial parent-child relationship
        service
            .with_client(TEST_CLIENT_ID)
            .move_node_unchecked(&child.id, Some(&parent1.id), None)
            .await?;

        // Subscribe to events (AFTER setup)
        let mut rx = service.subscribe_to_events();

        // Move child to new parent
        service
            .with_client(TEST_CLIENT_ID)
            .move_node_unchecked(&child.id, Some(&parent2.id), None)
            .await?;

        // Receive the emitted event (Issue #811: unified relationship events)
        let event = timeout(Duration::from_secs(1), rx.recv())
            .await
            .expect("Event should be emitted within 1 second")
            .expect("Should receive event");

        // Verify it's a RelationshipUpdated event for has_child with correct client_id
        match event {
            DomainEvent::RelationshipUpdated {
                relationship,
                source_client_id,
            } => {
                assert_eq!(relationship.relationship_type, "has_child");
                assert_eq!(relationship.from_id, parent2.id);
                assert_eq!(relationship.to_id, child.id);
                assert_eq!(source_client_id, Some(TEST_CLIENT_ID.to_string()));
            }
            _ => panic!("Expected RelationshipUpdated event, got {:?}", event),
        }

        Ok(())
    }

    #[tokio::test]
    async fn test_create_mention_emits_relationship_created_event() -> Result<()> {
        let (service, _temp_dir) = create_test_service().await?;

        // Create two nodes
        let source_node = create_root_node(&service, "text").await?;
        let target_node = create_root_node(&service, "text").await?;

        // Subscribe to events
        let mut rx = service.subscribe_to_events();

        // Create mention
        service
            .with_client(TEST_CLIENT_ID)
            .create_mention(&source_node.id, &target_node.id)
            .await?;

        // Receive the emitted event (Issue #811: unified relationship events)
        let event = timeout(Duration::from_secs(1), rx.recv())
            .await
            .expect("Event should be emitted within 1 second")
            .expect("Should receive event");

        // Verify it's a RelationshipCreated event for mentions with correct client_id
        match event {
            DomainEvent::RelationshipCreated {
                relationship,
                source_client_id,
            } => {
                assert_eq!(relationship.relationship_type, "mentions");
                assert_eq!(relationship.from_id, source_node.id);
                assert_eq!(relationship.to_id, target_node.id);
                assert_eq!(source_client_id, Some(TEST_CLIENT_ID.to_string()));
            }
            _ => panic!("Expected RelationshipCreated event, got {:?}", event),
        }

        Ok(())
    }

    #[tokio::test]
    async fn test_delete_mention_emits_relationship_deleted_event() -> Result<()> {
        let (service, _temp_dir) = create_test_service().await?;

        // Create two nodes and a mention
        let source_node = create_root_node(&service, "text").await?;
        let target_node = create_root_node(&service, "text").await?;
        service
            .with_client(TEST_CLIENT_ID)
            .create_mention(&source_node.id, &target_node.id)
            .await?;

        // Subscribe to events (AFTER setup)
        let mut rx = service.subscribe_to_events();

        // Delete mention
        service
            .with_client(TEST_CLIENT_ID)
            .remove_mention(&source_node.id, &target_node.id)
            .await?;

        // Receive the emitted event (Issue #811: unified relationship events)
        let event = timeout(Duration::from_secs(1), rx.recv())
            .await
            .expect("Event should be emitted within 1 second")
            .expect("Should receive event");

        // Verify it's a RelationshipDeleted event with correct client_id
        // Issue #813: Relationship IDs are now from universal `relationship` table
        match event {
            DomainEvent::RelationshipDeleted {
                id,
                from_id,
                to_id,
                relationship_type,
                source_client_id,
            } => {
                // Universal relationship table IDs contain "relationship:"
                assert!(
                    id.contains("relationship:"),
                    "Expected relationship table ID, got: {}",
                    id
                );
                assert_eq!(from_id, source_node.id);
                assert_eq!(to_id, target_node.id);
                assert_eq!(relationship_type, "mentions");
                assert_eq!(source_client_id, Some(TEST_CLIENT_ID.to_string()));
            }
            _ => panic!("Expected RelationshipDeleted event, got {:?}", event),
        }

        Ok(())
    }

    #[tokio::test]
    async fn test_only_one_event_emitted_per_operation() -> Result<()> {
        let (service, _temp_dir) = create_test_service().await?;

        // Create a node
        let node = create_root_node(&service, "text").await?;
        let node_id = node.id.clone();

        // Subscribe to events
        let mut rx = service.subscribe_to_events();

        // Update the node
        service
            .with_client(TEST_CLIENT_ID)
            .update_node_unchecked(
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

        assert!(matches!(event1, DomainEvent::NodeUpdated { .. }));

        // Attempting to receive another event should timeout
        let result = timeout(Duration::from_millis(100), rx.recv()).await;
        assert!(
            result.is_err(),
            "Should not receive a second event for single update"
        );

        Ok(())
    }

    #[tokio::test]
    async fn test_event_without_client_has_none_source_client_id() -> Result<()> {
        let (service, _temp_dir) = create_test_service().await?;

        // Subscribe to events
        let mut rx = service.subscribe_to_events();

        // Create a node WITHOUT setting client_id
        let node = Node::new("text".to_string(), "Test content".to_string(), json!({}));
        service.create_node(node).await?;

        // Receive the emitted event
        let event = timeout(Duration::from_secs(1), rx.recv())
            .await
            .expect("Event should be emitted within 1 second")
            .expect("Should receive event");

        // Verify source_client_id is None when not set
        match event {
            DomainEvent::NodeCreated {
                source_client_id, ..
            } => {
                assert_eq!(
                    source_client_id, None,
                    "source_client_id should be None when not set"
                );
            }
            _ => panic!("Expected NodeCreated event, got {:?}", event),
        }

        Ok(())
    }
}
