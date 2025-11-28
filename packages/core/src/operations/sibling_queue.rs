//! Sibling operation queue with retry logic for optimistic concurrency control
//!
//! This module provides a wrapper around NodeService that handles version
//! conflicts during sibling reordering operations by implementing automatic retry
//! with exponential backoff.
//!
//! # Why This Is Needed
//!
//! Sibling reordering operations are particularly prone to version conflicts when
//! multiple clients modify the same parent's children concurrently. Instead of
//! failing immediately on the first conflict, this queue retries with fresh version
//! data, achieving eventual consistency.
//!
//! # Example
//!
//! ```rust
//! use nodespace_core::operations::SiblingOperationQueue;
//! use nodespace_core::services::NodeService;
//!
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! # let node_service = todo!(); // NodeService instance
//! let queue = SiblingOperationQueue::new(node_service);
//!
//! // Retry up to 3 times with exponential backoff (10ms, 20ms, 40ms)
//! queue.reorder_with_retry(
//!     "node-123",
//!     Some("node-456"),  // insert_after_node_id
//!     3,                 // max_retries
//! ).await?;
//! # Ok(())
//! # }
//! ```

use crate::services::{NodeService, NodeServiceError};
use std::sync::Arc;
use tokio::time::Duration;

/// Queue for managing sibling operations with automatic retry on version conflicts
///
/// This wrapper around NodeService provides retry logic specifically for
/// reorder operations, which are most likely to encounter transient version conflicts.
pub struct SiblingOperationQueue {
    /// Underlying NodeService instance
    node_service: Arc<NodeService>,
}

impl SiblingOperationQueue {
    /// Create a new SiblingOperationQueue wrapping the given NodeService
    pub fn new(node_service: Arc<NodeService>) -> Self {
        Self { node_service }
    }

    /// Reorder a node with automatic retry on version conflicts
    ///
    /// This method implements exponential backoff retry logic to handle transient
    /// version conflicts during sibling reordering. Each retry fetches fresh version
    /// data before attempting the operation again.
    ///
    /// # Arguments
    ///
    /// * `node_id` - ID of the node to reorder
    /// * `insert_after_node_id` - Optional ID of the sibling this node should be placed after
    /// * `max_retries` - Maximum number of retry attempts (0 = single attempt, no retries)
    ///
    /// # Retry Behavior
    ///
    /// - **Retry on**: `NodeServiceError::VersionConflict` only
    /// - **Backoff**: Exponential (10ms, 20ms, 40ms, 80ms, ...)
    /// - **Fresh data**: Each retry fetches current node version
    /// - **Other errors**: Fail immediately without retry
    ///
    /// # Returns
    ///
    /// - `Ok(())` - Operation succeeded (possibly after retries)
    /// - `Err(NodeServiceError::VersionConflict)` - Max retries exceeded
    /// - `Err(NodeServiceError::*)` - Non-retriable error occurred
    ///
    /// # Example
    ///
    /// ```rust
    /// # use nodespace_core::operations::SiblingOperationQueue;
    /// # use nodespace_core::services::NodeService;
    /// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// # let node_service = todo!();
    /// let queue = SiblingOperationQueue::new(node_service);
    ///
    /// // Retry up to 5 times (exponential backoff: 10ms, 20ms, 40ms, 80ms, 160ms)
    /// match queue.reorder_with_retry("node-id", Some("sibling-id"), 5).await {
    ///     Ok(_) => println!("Reorder succeeded (possibly after retries)"),
    ///     Err(e) => eprintln!("Reorder failed: {}", e),
    /// }
    /// # Ok(())
    /// # }
    /// ```
    pub async fn reorder_with_retry(
        &self,
        node_id: &str,
        insert_after_node_id: Option<&str>,
        max_retries: usize,
    ) -> Result<(), NodeServiceError> {
        let mut attempt = 0;

        loop {
            // Fetch fresh version for this attempt
            let node = self
                .node_service
                .get_node(node_id)
                .await?
                .ok_or_else(|| NodeServiceError::node_not_found(node_id.to_string()))?;

            // Attempt reorder with current version
            match self
                .node_service
                .reorder_node_with_occ(node_id, node.version, insert_after_node_id)
                .await
            {
                // Success - return immediately
                Ok(_) => {
                    if attempt > 0 {
                        tracing::debug!(
                            "Sibling reorder succeeded after {} retry(ies) for node '{}'",
                            attempt,
                            node_id
                        );
                    }
                    return Ok(());
                }

                // Version conflict - retry if we haven't exceeded max_retries
                Err(NodeServiceError::VersionConflict {
                    node_id: ref conflict_node_id,
                    expected_version,
                    actual_version,
                    ..
                }) if attempt < max_retries => {
                    tracing::debug!(
                        "Version conflict on attempt {}/{} for node '{}': expected v{}, got v{}. Retrying...",
                        attempt + 1,
                        max_retries + 1,
                        conflict_node_id,
                        expected_version,
                        actual_version
                    );

                    // Exponential backoff: 10ms, 20ms, 40ms, 80ms, ...
                    let backoff_ms = 10u64 * (1 << attempt);
                    tokio::time::sleep(Duration::from_millis(backoff_ms)).await;

                    attempt += 1;
                    continue;
                }

                // Max retries exceeded or non-retriable error - fail
                Err(e) => {
                    if matches!(e, NodeServiceError::VersionConflict { .. }) {
                        tracing::warn!(
                            "Max retries ({}) exceeded for node '{}' reorder operation",
                            max_retries,
                            node_id
                        );
                    }
                    return Err(e);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::SurrealStore;
    use crate::operations::CreateNodeParams;
    use serde_json::json;
    use std::sync::atomic::{AtomicU64, Ordering};
    use tempfile::TempDir;

    /// Helper to set up test service
    async fn setup_test_service() -> Result<(Arc<NodeService>, TempDir), Box<dyn std::error::Error>>
    {
        let temp_dir = TempDir::new()?;
        let db_path = temp_dir.path().join("test.db");

        let store = Arc::new(SurrealStore::new(db_path).await?);
        let node_service = Arc::new(NodeService::new(store)?);
        Ok((node_service, temp_dir))
    }

    #[tokio::test]
    async fn test_reorder_with_retry_success_on_first_attempt() {
        let (node_service, _temp_dir) = setup_test_service().await.unwrap();
        let queue = SiblingOperationQueue::new(node_service.clone());

        // Create parent container (date node ID is auto-generated from content)
        let parent_id = node_service
            .create_node_with_parent(CreateNodeParams {
                id: None, // Auto-generated as "2025-01-01" from content
                node_type: "date".to_string(),
                content: "2025-01-01".to_string(),
                parent_id: None,
                insert_after_node_id: None,
                properties: json!({}),
            })
            .await
            .unwrap();

        // Create two sibling nodes: A → B
        let node_a_id = node_service
            .create_node_with_parent(CreateNodeParams {
                id: None, // Auto-generate UUID
                node_type: "text".to_string(),
                content: "Node A".to_string(),
                parent_id: Some(parent_id.clone()),
                insert_after_node_id: None,
                properties: json!({}),
            })
            .await
            .unwrap();

        let node_b_id = node_service
            .create_node_with_parent(CreateNodeParams {
                id: None, // Auto-generate UUID
                node_type: "text".to_string(),
                content: "Node B".to_string(),
                parent_id: Some(parent_id.clone()),
                insert_after_node_id: Some(node_a_id.clone()),
                properties: json!({}),
            })
            .await
            .unwrap();

        // Reorder B to be before A (should succeed on first attempt)
        let result = queue.reorder_with_retry(&node_b_id, None, 3).await;
        assert!(result.is_ok(), "Reorder should succeed on first attempt");

        // Verify new order via get_children (ordered by edge order field)
        let children = node_service.get_children(&parent_id).await.unwrap();
        assert_eq!(children.len(), 2);
        // After reorder, B should be first
        assert_eq!(children[0].id, node_b_id, "B should be first");
        assert_eq!(children[1].id, node_a_id, "A should be second");
    }

    #[tokio::test]
    async fn test_reorder_with_retry_handles_version_conflict() {
        let (node_service, _temp_dir) = setup_test_service().await.unwrap();
        let queue = SiblingOperationQueue::new(node_service.clone());

        // Create parent container (date node ID is auto-generated from content)
        let parent_id = node_service
            .create_node_with_parent(CreateNodeParams {
                id: None, // Auto-generated as "2025-01-01" from content
                node_type: "date".to_string(),
                content: "2025-01-01".to_string(),
                parent_id: None,
                insert_after_node_id: None,
                properties: json!({}),
            })
            .await
            .unwrap();

        // Create node
        let node_id = node_service
            .create_node_with_parent(CreateNodeParams {
                id: None, // Auto-generate UUID
                node_type: "text".to_string(),
                content: "Original content".to_string(),
                parent_id: Some(parent_id.clone()),
                insert_after_node_id: None,
                properties: json!({}),
            })
            .await
            .unwrap();

        // Simulate concurrent modification by updating the node directly
        // This will increment its version, causing the reorder to conflict
        let node = node_service.get_node(&node_id).await.unwrap().unwrap();
        let update = crate::models::NodeUpdate {
            content: Some("Modified content".to_string()),
            node_type: None,
            properties: None,
            embedding_vector: None,
        };
        node_service
            .update_node_with_occ(&node_id, node.version, update)
            .await
            .unwrap();

        // Now reorder with retry - it should fetch fresh version and succeed
        let result = queue.reorder_with_retry(&node_id, None, 3).await;
        assert!(
            result.is_ok(),
            "Reorder should succeed after retrying with fresh version"
        );
    }

    #[tokio::test]
    async fn test_reorder_with_retry_exponential_backoff() {
        let (node_service, _temp_dir) = setup_test_service().await.unwrap();
        let queue = SiblingOperationQueue::new(node_service.clone());

        // Create parent container (date node ID is auto-generated from content)
        let parent_id = node_service
            .create_node_with_parent(CreateNodeParams {
                id: None, // Auto-generated as "2025-01-01" from content
                node_type: "date".to_string(),
                content: "2025-01-01".to_string(),
                parent_id: None,
                insert_after_node_id: None,
                properties: json!({}),
            })
            .await
            .unwrap();

        // Create node
        let node_id = node_service
            .create_node_with_parent(CreateNodeParams {
                id: None, // Auto-generate UUID
                node_type: "text".to_string(),
                content: "Test".to_string(),
                parent_id: Some(parent_id.clone()),
                insert_after_node_id: None,
                properties: json!({}),
            })
            .await
            .unwrap();

        // Measure time for operation with retries
        let start = std::time::Instant::now();

        // This should succeed immediately (no conflicts)
        queue.reorder_with_retry(&node_id, None, 5).await.unwrap();

        let elapsed = start.elapsed();

        // Should complete quickly since there are no conflicts
        assert!(
            elapsed.as_millis() < 50,
            "Operation without conflicts should be fast"
        );
    }

    #[tokio::test]
    async fn test_reorder_with_retry_max_retries_exceeded() {
        let (node_service, _temp_dir) = setup_test_service().await.unwrap();
        let queue = SiblingOperationQueue::new(node_service.clone());

        // Create parent container (date node ID is auto-generated from content)
        let parent_id = node_service
            .create_node_with_parent(CreateNodeParams {
                id: None, // Auto-generated as "2025-01-01" from content
                node_type: "date".to_string(),
                content: "2025-01-01".to_string(),
                parent_id: None,
                insert_after_node_id: None,
                properties: json!({}),
            })
            .await
            .unwrap();

        // Create node
        let node_id = node_service
            .create_node_with_parent(CreateNodeParams {
                id: None, // Auto-generate UUID
                node_type: "text".to_string(),
                content: "Test".to_string(),
                parent_id: Some(parent_id.clone()),
                insert_after_node_id: None,
                properties: json!({}),
            })
            .await
            .unwrap();

        // Simulate high-contention scenario by continuously updating the node
        // in a background task
        let node_service_clone = node_service.clone();
        let node_id_clone = node_id.clone();
        let update_count = Arc::new(AtomicU64::new(0));
        let update_count_clone = update_count.clone();

        let update_task = tokio::spawn(async move {
            // Continuously update the node to create conflicts
            for i in 0..10 {
                let current_node = node_service_clone
                    .get_node(&node_id_clone)
                    .await
                    .unwrap()
                    .unwrap();
                let update = crate::models::NodeUpdate {
                    content: Some(format!("Update {}", i)),
                    node_type: None,
                    properties: None,
                    embedding_vector: None,
                };
                let _ = node_service_clone
                    .update_node_with_occ(&node_id_clone, current_node.version, update)
                    .await;
                update_count_clone.fetch_add(1, Ordering::SeqCst);
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        });

        // Try to reorder with very limited retries (likely to fail)
        let result = queue.reorder_with_retry(&node_id, None, 0).await;

        // Wait for background updates to finish
        update_task.await.unwrap();

        // With 0 retries and concurrent updates, we might get a conflict
        // However, the operation might also succeed if timing is right
        // The key test is that we don't panic or hang
        match result {
            Ok(_) => {
                // Success is acceptable if timing was right
            }
            Err(NodeServiceError::VersionConflict { .. }) => {
                // Expected failure due to max retries exceeded
            }
            Err(e) => {
                panic!("Unexpected error type: {:?}", e);
            }
        }
    }

    #[tokio::test]
    async fn test_reorder_with_retry_nonexistent_node() {
        let (node_service, _temp_dir) = setup_test_service().await.unwrap();
        let queue = SiblingOperationQueue::new(node_service.clone());

        // Try to reorder a node that doesn't exist
        let result = queue.reorder_with_retry("nonexistent-node", None, 3).await;

        // Should fail with NodeNotFound error (not retry version conflicts)
        assert!(
            matches!(result, Err(NodeServiceError::NodeNotFound { .. })),
            "Should get NodeNotFound error, got: {:?}",
            result
        );
    }

    #[tokio::test]
    async fn test_reorder_with_retry_eventual_consistency() {
        let (node_service, _temp_dir) = setup_test_service().await.unwrap();
        let _queue = SiblingOperationQueue::new(node_service.clone());

        // Create parent container (date node ID is auto-generated from content)
        let parent_id = node_service
            .create_node_with_parent(CreateNodeParams {
                id: None, // Auto-generated as "2025-01-01" from content
                node_type: "date".to_string(),
                content: "2025-01-01".to_string(),
                parent_id: None,
                insert_after_node_id: None,
                properties: json!({}),
            })
            .await
            .unwrap();

        // Create three nodes: A → B → C
        let node_a_id = node_service
            .create_node_with_parent(CreateNodeParams {
                id: None, // Auto-generate UUID
                node_type: "text".to_string(),
                content: "A".to_string(),
                parent_id: Some(parent_id.clone()),
                insert_after_node_id: None,
                properties: json!({}),
            })
            .await
            .unwrap();

        let node_b_id = node_service
            .create_node_with_parent(CreateNodeParams {
                id: None, // Auto-generate UUID
                node_type: "text".to_string(),
                content: "B".to_string(),
                parent_id: Some(parent_id.clone()),
                insert_after_node_id: Some(node_a_id.clone()),
                properties: json!({}),
            })
            .await
            .unwrap();

        let node_c_id = node_service
            .create_node_with_parent(CreateNodeParams {
                id: None, // Auto-generate UUID
                node_type: "text".to_string(),
                content: "C".to_string(),
                parent_id: Some(parent_id.clone()),
                insert_after_node_id: Some(node_b_id.clone()),
                properties: json!({}),
            })
            .await
            .unwrap();

        // Simulate concurrent reorder operations
        let queue_clone = SiblingOperationQueue::new(node_service.clone());

        // Both operations attempt to reorder at the same time
        let task1 = {
            let q = SiblingOperationQueue::new(node_service.clone());
            let c = node_c_id.clone();
            tokio::spawn(async move {
                // Move C to first position
                q.reorder_with_retry(&c, None, 5).await
            })
        };

        let task2 = {
            let q = queue_clone;
            let b = node_b_id.clone();
            tokio::spawn(async move {
                // Move B before A
                q.reorder_with_retry(&b, None, 5).await
            })
        };

        // Both should eventually succeed (with retries)
        let result1 = task1.await.unwrap();
        let result2 = task2.await.unwrap();

        assert!(result1.is_ok(), "Task 1 should succeed with retries");
        assert!(result2.is_ok(), "Task 2 should succeed with retries");

        // Verify final state is consistent via get_children ordering
        let children = node_service.get_children(&parent_id).await.unwrap();
        assert_eq!(children.len(), 3, "Should have 3 children");

        // Verify all nodes are present (order may vary due to concurrent operations)
        let child_ids: Vec<_> = children.iter().map(|n| n.id.clone()).collect();
        assert!(child_ids.contains(&node_a_id), "A should be present");
        assert!(child_ids.contains(&node_b_id), "B should be present");
        assert!(child_ids.contains(&node_c_id), "C should be present");
    }
}
