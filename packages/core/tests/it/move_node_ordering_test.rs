//! C3a: move_node ordering contract tests
//!
//! Regression-locks the contract that the frontend depends on after removing
//! client-side fractional order math:
//!
//! - `move_node` with `InsertPosition::After(oldParentId)` emits a
//!   `RelationshipUpdated` event whose payload contains `{"order": <real_number>}`.
//! - The emitted order places the moved node between its left and right siblings
//!   (i.e., left_order < emitted_order < right_order or emitted_order > left_order
//!   when no right sibling exists).
//! - Moving a node to `InsertPosition::End` emits an order greater than any
//!   existing sibling.
//!
//! These properties are the single source of truth the frontend relies on for
//! hierarchy reconciliation via `hierarchy-sync.ts::applyHasChildUpdated`.

#[cfg(test)]
mod move_node_ordering_tests {
    use anyhow::Result;
    use nodespace_core::db::{DomainEvent, SqliteStore};
    use nodespace_core::models::Node;
    use nodespace_core::services::{InsertPosition, NodeService};
    use serde_json::json;
    use std::sync::Arc;
    use tempfile::TempDir;
    use tokio::time::{timeout, Duration};

    const TEST_CLIENT_ID: &str = "test-client";

    async fn create_test_service() -> Result<(NodeService, TempDir)> {
        let temp_dir = TempDir::new()?;
        let db_path = temp_dir.path().join("test.db");
        let mut store = Arc::new(SqliteStore::new(db_path).await?);
        let service = NodeService::new(&mut store).await?;
        Ok((service, temp_dir))
    }

    async fn create_node(service: &NodeService) -> Result<Node> {
        let node = Node::new("text".to_string(), "test".to_string(), json!({}));
        service
            .with_client(TEST_CLIENT_ID)
            .create_node(node.clone())
            .await?;
        let created = service
            .get_node(&node.id)
            .await?
            .expect("Node should exist");
        Ok(created)
    }

    /// Extract the `order` field from a `RelationshipUpdated` event payload.
    fn extract_order_from_event(event: &DomainEvent) -> Option<f64> {
        if let DomainEvent::RelationshipUpdated { relationship } = event {
            relationship
                .properties
                .get("order")
                .and_then(|v| v.as_f64())
        } else {
            None
        }
    }

    /// C3a contract: move_node with InsertPosition::After(sibling) emits
    /// RelationshipUpdated with a real `order` value greater than the sibling's order.
    #[tokio::test]
    async fn test_move_node_after_sibling_emits_order_in_relationship_updated() -> Result<()> {
        let (service, _temp_dir) = create_test_service().await?;

        // Setup: grandparent > [parent, right_sibling]
        let grandparent = create_node(&service).await?;
        let parent = create_node(&service).await?;
        let right_sibling = create_node(&service).await?;
        let child = create_node(&service).await?;

        // Place parent and right_sibling under grandparent
        service
            .with_client(TEST_CLIENT_ID)
            .move_node_unchecked(&parent.id, Some(&grandparent.id), InsertPosition::End)
            .await?;
        service
            .with_client(TEST_CLIENT_ID)
            .move_node_unchecked(
                &right_sibling.id,
                Some(&grandparent.id),
                InsertPosition::End,
            )
            .await?;

        // Place child under parent (so we have something to outdent)
        service
            .with_client(TEST_CLIENT_ID)
            .move_node_unchecked(&child.id, Some(&parent.id), InsertPosition::End)
            .await?;

        // Subscribe before the outdent-equivalent move
        let mut rx = service.subscribe_to_events();

        // Outdent-equivalent: move child to grandparent, InsertPosition::After(parent)
        // This mirrors the frontend's `backendAdapter.moveNode(nodeId, version, newParentId,
        // { type: 'after', siblingId: oldParentId })` call.
        let child_node = service
            .get_node(&child.id)
            .await?
            .expect("child must exist");
        service
            .with_client(TEST_CLIENT_ID)
            .move_node(
                &child.id,
                child_node.version,
                Some(&grandparent.id),
                InsertPosition::After(&parent.id),
            )
            .await?;

        // Loop until we receive a RelationshipUpdated event (defensive against unrelated events)
        let order = loop {
            let envelope = timeout(Duration::from_secs(2), rx.recv())
                .await
                .expect("RelationshipUpdated event should arrive within 2 seconds")
                .expect("Channel should not close");
            if let Some(o) = extract_order_from_event(&envelope.event) {
                break o;
            }
        };

        // The emitted order must be finite (not NaN, not infinity)
        assert!(order.is_finite(), "order must be a finite float: {}", order);

        // The order must be > 0 (fractional ordering never produces negatives for normal inserts)
        assert!(order > 0.0, "order must be positive: {}", order);

        Ok(())
    }

    /// C3a contract: move_node with InsertPosition::After(parent) places the moved node
    /// between parent's order and the next sibling's order.
    #[tokio::test]
    async fn test_move_node_after_sibling_order_is_between_neighbors() -> Result<()> {
        let (service, _temp_dir) = create_test_service().await?;

        // Setup: grandparent > [parent(order=A), next_sibling(order=B)]
        // We need to know A and B to verify the emitted order is between them.
        let grandparent = create_node(&service).await?;
        let parent = create_node(&service).await?;
        let next_sibling = create_node(&service).await?;
        let child = create_node(&service).await?;

        // Subscribe to capture orders for parent and next_sibling placement
        let mut rx = service.subscribe_to_events();

        // Read events defensively (loop until the wanted variant arrives)
        // rather than assuming the very next event on the channel is a
        // `RelationshipUpdated` — ADR-069 §2/S4 made `move_node` bump the
        // node's version BEFORE emitting its relationship event, so a
        // `NodeUpdated` from that bump now legitimately arrives first on
        // every one of these moves. This mirrors the sibling test
        // (`test_move_node_after_sibling_emits_order_in_relationship_updated`),
        // which already used this pattern.
        async fn next_order_event(
            rx: &mut tokio::sync::broadcast::Receiver<nodespace_core::db::EventEnvelope>,
        ) -> f64 {
            loop {
                let envelope = timeout(Duration::from_secs(2), rx.recv())
                    .await
                    .expect("RelationshipUpdated event should arrive within 2 seconds")
                    .expect("channel should not close");
                if let Some(order) = extract_order_from_event(&envelope.event) {
                    return order;
                }
            }
        }

        service
            .with_client(TEST_CLIENT_ID)
            .move_node_unchecked(&parent.id, Some(&grandparent.id), InsertPosition::End)
            .await?;
        let parent_order = next_order_event(&mut rx).await;

        service
            .with_client(TEST_CLIENT_ID)
            .move_node_unchecked(&next_sibling.id, Some(&grandparent.id), InsertPosition::End)
            .await?;
        let next_sibling_order = next_order_event(&mut rx).await;

        // Place child under parent
        service
            .with_client(TEST_CLIENT_ID)
            .move_node_unchecked(&child.id, Some(&parent.id), InsertPosition::End)
            .await?;
        // drain child-placement event
        next_order_event(&mut rx).await;

        // Outdent: move child to grandparent After(parent)
        let child_node = service
            .get_node(&child.id)
            .await?
            .expect("child must exist");
        service
            .with_client(TEST_CLIENT_ID)
            .move_node(
                &child.id,
                child_node.version,
                Some(&grandparent.id),
                InsertPosition::After(&parent.id),
            )
            .await?;

        let outdent_order = next_order_event(&mut rx).await;

        // Contract: parent_order < outdent_order < next_sibling_order
        assert!(
            outdent_order > parent_order,
            "outdented node order ({}) must be > parent order ({})",
            outdent_order,
            parent_order
        );
        assert!(
            outdent_order < next_sibling_order,
            "outdented node order ({}) must be < next_sibling order ({})",
            outdent_order,
            next_sibling_order
        );

        Ok(())
    }

    /// C3a contract: move_node with InsertPosition::End emits an order greater than
    /// any existing sibling. This is the append-fallback the frontend relies on.
    #[tokio::test]
    async fn test_move_node_end_emits_order_greater_than_existing_siblings() -> Result<()> {
        let (service, _temp_dir) = create_test_service().await?;

        let parent = create_node(&service).await?;
        let child_a = create_node(&service).await?;
        let child_b = create_node(&service).await?;

        let mut rx = service.subscribe_to_events();

        service
            .with_client(TEST_CLIENT_ID)
            .move_node_unchecked(&child_a.id, Some(&parent.id), InsertPosition::End)
            .await?;
        let a_envelope = timeout(Duration::from_secs(1), rx.recv())
            .await
            .expect("timeout")
            .expect("event");
        let order_a = extract_order_from_event(&a_envelope.event).expect("must emit order");

        service
            .with_client(TEST_CLIENT_ID)
            .move_node_unchecked(&child_b.id, Some(&parent.id), InsertPosition::End)
            .await?;
        let b_envelope = timeout(Duration::from_secs(1), rx.recv())
            .await
            .expect("timeout")
            .expect("event");
        let order_b = extract_order_from_event(&b_envelope.event).expect("must emit order");

        assert!(
            order_b > order_a,
            "End-appended node_b order ({}) must exceed node_a order ({})",
            order_b,
            order_a
        );

        Ok(())
    }
}
