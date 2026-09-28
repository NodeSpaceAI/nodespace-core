//! Event Emission Tests
//!
//! Tests that verify correct event emission for all major operations.
//! Events are now emitted at the NodeService layer
//! (not SqliteStore) to support client filtering.
//! Events are wrapped in EventEnvelope with metadata.
//!
//! These tests verify:
//! 1. Correct events are emitted for each operation type
//! 2. Events contain proper source_client_id in envelope metadata when set via with_client()
//! 3. Events are emitted AFTER the transaction completes successfully

#[cfg(test)]
mod event_emission_tests {
    use anyhow::Result;
    use nodespace_core::db::{DomainEvent, SqliteStore};
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
        let mut store = Arc::new(SqliteStore::new(db_path).await?);
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

        // Receive the emitted event (EventEnvelope)
        let envelope = timeout(Duration::from_secs(1), rx.recv())
            .await
            .expect("Event should be emitted within 1 second")
            .expect("Should receive event");

        // Verify it's a NodeCreated event with correct client_id in envelope metadata
        // (ID-only events, node_type, EventEnvelope)
        match &envelope.event {
            DomainEvent::NodeCreated { node_id, node_type } => {
                assert_eq!(node_id, &expected_id);
                assert_eq!(node_type, "text");
                assert_eq!(
                    envelope.metadata.source_client_id,
                    Some(TEST_CLIENT_ID.to_string())
                );
            }
            _ => panic!("Expected NodeCreated event, got {:?}", envelope.event),
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

        // Receive the emitted event (EventEnvelope)
        let envelope = timeout(Duration::from_secs(1), rx.recv())
            .await
            .expect("Event should be emitted within 1 second")
            .expect("Should receive event");

        // Verify it's a NodeUpdated event with correct client_id in envelope metadata
        // (ID-only events, EventEnvelope + node_type + changed_properties)
        match &envelope.event {
            DomainEvent::NodeUpdated {
                node_id: updated_id,
                ..
            } => {
                assert_eq!(updated_id, &node_id);
                assert_eq!(
                    envelope.metadata.source_client_id,
                    Some(TEST_CLIENT_ID.to_string())
                );
            }
            _ => panic!("Expected NodeUpdated event, got {:?}", envelope.event),
        }

        Ok(())
    }

    #[tokio::test]
    async fn test_collection_query_paginates_members_not_global_set() -> Result<()> {
        // offset/limit must paginate the COLLECTION'S members (in memory),
        // not the global unfiltered set, and non-members must be excluded.
        let (service, _temp_dir) = create_test_service().await?;

        let coll = nodespace_core::models::Node::new(
            "collection".to_string(),
            "Team".to_string(),
            json!({}),
        );
        let coll_id = coll.id.clone();
        service.create_node(coll).await?;

        // 3 members + 2 non-members (never added to the collection).
        for i in 0..3 {
            let m =
                nodespace_core::models::Node::new("text".to_string(), format!("m{i}"), json!({}));
            let mid = m.id.clone();
            service.create_node(m).await?;
            service
                .store()
                .add_to_collection(&mid, &coll_id, &json!({}))
                .await?;
        }
        for i in 0..2 {
            let other =
                nodespace_core::models::Node::new("text".to_string(), format!("x{i}"), json!({}));
            service.create_node(other).await?;
        }

        let service = Arc::new(service);
        let query = |offset: Option<usize>, limit: Option<usize>| {
            let svc = service.clone();
            let coll_id = coll_id.clone();
            async move {
                nodespace_core::ops::node_ops::query_nodes(
                    &svc,
                    nodespace_core::ops::node_ops::QueryNodesInput {
                        node_type: None,
                        limit,
                        offset,
                        collection_id: Some(coll_id),
                        collection: None,
                        filters: None,
                    },
                )
                .await
            }
        };

        // All 3 members (non-members excluded).
        assert_eq!(query(None, Some(10)).await?.count, 3);
        // offset skips members, not the global set.
        assert_eq!(query(Some(1), Some(10)).await?.count, 2);
        assert_eq!(query(Some(1), Some(1)).await?.count, 1);
        // offset past the end → empty.
        assert_eq!(query(Some(10), Some(10)).await?.count, 0);
        Ok(())
    }

    #[tokio::test]
    async fn test_collection_query_returns_members_despite_many_newer_nonmembers() -> Result<()> {
        // Regression guard: members must be returned even when MANY newer
        // non-member nodes exist. The SQL is id-scoped to the member set, so it can
        // never be crowded out. This FAILS under a capped-global-limit approach
        // (the newest N rows would all be non-members → 0 members) and under the
        // old fixed-1000 over-fetch (members beyond the newest 1000 dropped).
        let (service, _temp_dir) = create_test_service().await?;

        let coll = nodespace_core::models::Node::new(
            "collection".to_string(),
            "Team".to_string(),
            json!({}),
        );
        let coll_id = coll.id.clone();
        service.create_node(coll).await?;

        // 3 members created FIRST.
        for i in 0..3 {
            let m =
                nodespace_core::models::Node::new("text".to_string(), format!("m{i}"), json!({}));
            let mid = m.id.clone();
            service.create_node(m).await?;
            service
                .store()
                .add_to_collection(&mid, &coll_id, &json!({}))
                .await?;
        }
        // 25 NON-member nodes created AFTER (newer) — would dominate a global limit.
        for i in 0..25 {
            service
                .create_node(nodespace_core::models::Node::new(
                    "text".to_string(),
                    format!("x{i}"),
                    json!({}),
                ))
                .await?;
        }

        let service = Arc::new(service);
        let out = nodespace_core::ops::node_ops::query_nodes(
            &service,
            nodespace_core::ops::node_ops::QueryNodesInput {
                node_type: None,
                limit: Some(10),
                offset: None,
                collection_id: Some(coll_id),
                collection: None,
                filters: None,
            },
        )
        .await?;
        // All 3 members returned (not crowded out by the 25 newer non-members).
        assert_eq!(out.count, 3);
        Ok(())
    }

    #[tokio::test]
    async fn test_bulk_update_emits_changed_properties_and_merges() -> Result<()> {
        // bulk_update must emit a NON-empty changed_properties (so
        // property-change automation fires) and MERGE properties (not wholesale
        // replace), matching the single-update path.
        let (service, _temp_dir) = create_test_service().await?;
        let node = create_root_node(&service, "text").await?;
        let node_id = node.id.clone();

        // Baseline property via the single-update path.
        service
            .with_client(TEST_CLIENT_ID)
            .update_node_unchecked(
                &node_id,
                nodespace_core::models::NodeUpdate {
                    properties: Some(json!({ "keep": "yes" })),
                    ..Default::default()
                },
            )
            .await?;

        let mut rx = service.subscribe_to_events();

        // Bulk-update a DIFFERENT property.
        service
            .with_client(TEST_CLIENT_ID)
            .bulk_update(vec![(
                node_id.clone(),
                nodespace_core::models::NodeUpdate {
                    properties: Some(json!({ "add": "new" })),
                    ..Default::default()
                },
            )])
            .await?;

        let envelope = timeout(Duration::from_secs(1), rx.recv())
            .await
            .expect("Event should be emitted within 1 second")
            .expect("Should receive event");
        match &envelope.event {
            DomainEvent::NodeUpdated {
                node_id: id,
                changed_properties,
                ..
            } => {
                assert_eq!(id, &node_id);
                assert!(
                    !changed_properties.is_empty(),
                    "bulk_update must emit changed_properties (was hardcoded empty)"
                );
            }
            _ => panic!("Expected NodeUpdated event, got {:?}", envelope.event),
        }

        // Merge, not wholesale replace — the pre-existing property survives.
        let updated = service.get_node(&node_id).await?.expect("node exists");
        let props = updated.properties.to_string();
        assert!(
            props.contains("keep") && props.contains("yes"),
            "pre-existing property must survive the bulk update (merge): {props}"
        );
        assert!(
            props.contains("add") && props.contains("new"),
            "new property must be present after the bulk update: {props}"
        );
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

        // Receive the emitted event (EventEnvelope)
        let envelope = timeout(Duration::from_secs(1), rx.recv())
            .await
            .expect("Event should be emitted within 1 second")
            .expect("Should receive event");

        // Verify it's a NodeDeleted event with correct client_id in envelope metadata
        match &envelope.event {
            DomainEvent::NodeDeleted { id, node_type: _ } => {
                assert_eq!(id, &node_id);
                assert_eq!(
                    envelope.metadata.source_client_id,
                    Some(TEST_CLIENT_ID.to_string())
                );
            }
            _ => panic!("Expected NodeDeleted event, got {:?}", envelope.event),
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
            .move_node_unchecked(
                &child.id,
                Some(&parent1.id),
                nodespace_core::services::InsertPosition::Beginning,
            )
            .await?;

        // Subscribe to events (AFTER setup)
        let mut rx = service.subscribe_to_events();

        // Move child to new parent
        service
            .with_client(TEST_CLIENT_ID)
            .move_node_unchecked(
                &child.id,
                Some(&parent2.id),
                nodespace_core::services::InsertPosition::Beginning,
            )
            .await?;

        // Receive the emitted event (unified relationship events, EventEnvelope)
        let envelope = timeout(Duration::from_secs(1), rx.recv())
            .await
            .expect("Event should be emitted within 1 second")
            .expect("Should receive event");

        // Verify it's a RelationshipUpdated event for has_child with correct client_id in metadata
        match &envelope.event {
            DomainEvent::RelationshipUpdated { relationship } => {
                assert_eq!(relationship.relationship_type, "has_child");
                assert_eq!(relationship.from_id, format!("node:{}", parent2.id));
                assert_eq!(relationship.to_id, format!("node:{}", child.id));
                assert_eq!(
                    envelope.metadata.source_client_id,
                    Some(TEST_CLIENT_ID.to_string())
                );
            }
            _ => panic!(
                "Expected RelationshipUpdated event, got {:?}",
                envelope.event
            ),
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

        // Receive the emitted event (unified relationship events, EventEnvelope)
        let envelope = timeout(Duration::from_secs(1), rx.recv())
            .await
            .expect("Event should be emitted within 1 second")
            .expect("Should receive event");

        // Verify it's a RelationshipCreated event for mentions with correct client_id in metadata
        match &envelope.event {
            DomainEvent::RelationshipCreated { relationship } => {
                assert_eq!(relationship.relationship_type, "mentions");
                assert_eq!(relationship.from_id, format!("node:{}", source_node.id));
                assert_eq!(relationship.to_id, format!("node:{}", target_node.id));
                assert_eq!(
                    envelope.metadata.source_client_id,
                    Some(TEST_CLIENT_ID.to_string())
                );
            }
            _ => panic!(
                "Expected RelationshipCreated event, got {:?}",
                envelope.event
            ),
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

        // Receive the emitted event (unified relationship events, EventEnvelope)
        let envelope = timeout(Duration::from_secs(1), rx.recv())
            .await
            .expect("Event should be emitted within 1 second")
            .expect("Should receive event");

        // Verify it's a RelationshipDeleted event with correct client_id in metadata
        // Relationship IDs are now from universal `relationship` table
        match &envelope.event {
            DomainEvent::RelationshipDeleted {
                id: _,
                from_id,
                to_id,
                relationship_type,
            } => {
                assert_eq!(from_id, &format!("node:{}", source_node.id));
                assert_eq!(to_id, &format!("node:{}", target_node.id));
                assert_eq!(relationship_type, "mentions");
                assert_eq!(
                    envelope.metadata.source_client_id,
                    Some(TEST_CLIENT_ID.to_string())
                );
            }
            _ => panic!(
                "Expected RelationshipDeleted event, got {:?}",
                envelope.event
            ),
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

        // Should receive exactly ONE event (EventEnvelope)
        let envelope1 = timeout(Duration::from_secs(1), rx.recv())
            .await
            .expect("Should receive event")
            .expect("Should receive event");

        assert!(matches!(envelope1.event, DomainEvent::NodeUpdated { .. }));

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

        // Receive the emitted event (EventEnvelope)
        let envelope = timeout(Duration::from_secs(1), rx.recv())
            .await
            .expect("Event should be emitted within 1 second")
            .expect("Should receive event");

        // Verify source_client_id is None in envelope metadata when not set
        match &envelope.event {
            DomainEvent::NodeCreated { .. } => {
                assert_eq!(
                    envelope.metadata.source_client_id, None,
                    "source_client_id should be None when not set"
                );
            }
            _ => panic!("Expected NodeCreated event, got {:?}", envelope.event),
        }

        Ok(())
    }
}
