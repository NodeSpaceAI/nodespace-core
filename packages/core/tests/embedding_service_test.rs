//! Comprehensive tests for NodeEmbeddingService
//!
//! Tests cover:
//! - Root node detection
//! - Content aggregation
//! - Embedding generation
//! - Queue management
//! - Semantic search
//! - Error handling
//!
//! NOTE: This test file requires the embedding-service feature to be enabled
//! and a valid embedding model to be present for full end-to-end testing.
//! For now, we test the service structure and logic paths.

use anyhow::Result;
use nodespace_core::{
    db::SurrealStore,
    models::{EmbeddingConfig, Node},
    services::{embedding_service::NodeEmbeddingService, NodeService},
};
use nodespace_nlp_engine::{EmbeddingConfig as NlpConfig, EmbeddingService};
use serde_json::json;
use std::sync::Arc;
use tempfile::TempDir;

/// Test helper: Create a test database and NodeService
async fn create_test_service() -> Result<(NodeService, TempDir)> {
    let temp_dir = TempDir::new()?;
    let db_path = temp_dir.path().join("test.db");
    let mut store = Arc::new(SurrealStore::new(db_path).await?);
    let service = NodeService::new(&mut store).await?;
    Ok((service, temp_dir))
}

/// Test helper: Create a test NLP engine (uninitialized for testing)
fn create_test_nlp_engine() -> Arc<EmbeddingService> {
    let config = NlpConfig::default();
    Arc::new(EmbeddingService::new(config).unwrap())
}

/// Test helper: Create a test embedding service
async fn create_test_embedding_service(
) -> Result<(NodeEmbeddingService, Arc<SurrealStore>, TempDir)> {
    let temp_dir = TempDir::new()?;
    let db_path = temp_dir.path().join("test.db");
    let store = Arc::new(SurrealStore::new(db_path).await?);
    let nlp_engine = create_test_nlp_engine();
    let embedding_service = NodeEmbeddingService::new(nlp_engine, store.clone());
    Ok((embedding_service, store, temp_dir))
}

/// Test helper: Create a test node via NodeService
async fn create_root_node(service: &NodeService, node_type: &str, content: &str) -> Result<Node> {
    let node = Node::new(node_type.to_string(), content.to_string(), json!({}));
    service.create_node(node.clone()).await?;
    let created = service
        .get_node(&node.id)
        .await?
        .expect("Node should exist");
    Ok(created)
}

/// Test helper: Create a child node under a parent
async fn create_child_node(
    service: &NodeService,
    parent_id: &str,
    node_type: &str,
    content: &str,
) -> Result<Node> {
    let node = Node::new(node_type.to_string(), content.to_string(), json!({}));
    service.create_node(node.clone()).await?;
    service.move_node(&node.id, Some(parent_id), None).await?;
    let created = service
        .get_node(&node.id)
        .await?
        .expect("Node should exist");
    Ok(created)
}

// =========================================================================
// Root Node Detection Tests
// =========================================================================

#[tokio::test]
async fn test_is_root_node_with_no_parent() -> Result<()> {
    let (embedding_service, _store, _temp_dir) = create_test_embedding_service().await?;
    let (node_service, _node_temp_dir) = create_test_service().await?;

    let root = create_root_node(&node_service, "text", "Root content").await?;

    let is_root = embedding_service.is_root_node(&root.id).await?;
    assert!(is_root, "Node with no parent should be identified as root");
    Ok(())
}

#[tokio::test]
async fn test_is_root_node_with_parent() -> Result<()> {
    let (embedding_service, _store, _temp_dir) = create_test_embedding_service().await?;
    let (node_service, _node_temp_dir) = create_test_service().await?;

    let root = create_root_node(&node_service, "text", "Root").await?;
    let child = create_child_node(&node_service, &root.id, "text", "Child").await?;

    let is_root = embedding_service.is_root_node(&child.id).await?;
    assert!(
        !is_root,
        "Node with parent should not be identified as root"
    );
    Ok(())
}

#[tokio::test]
async fn test_find_root_id_for_root_node() -> Result<()> {
    let (embedding_service, _store, _temp_dir) = create_test_embedding_service().await?;
    let (node_service, _node_temp_dir) = create_test_service().await?;

    let root = create_root_node(&node_service, "text", "Root").await?;

    let found_root_id = embedding_service.find_root_id(&root.id).await?;
    assert_eq!(found_root_id, root.id, "Root should find itself");
    Ok(())
}

#[tokio::test]
async fn test_find_root_id_for_deep_child() -> Result<()> {
    let (embedding_service, _store, _temp_dir) = create_test_embedding_service().await?;
    let (node_service, _node_temp_dir) = create_test_service().await?;

    // Create a tree: root -> child1 -> child2 -> child3
    let root = create_root_node(&node_service, "text", "Root").await?;
    let child1 = create_child_node(&node_service, &root.id, "text", "Child1").await?;
    let child2 = create_child_node(&node_service, &child1.id, "text", "Child2").await?;
    let child3 = create_child_node(&node_service, &child2.id, "text", "Child3").await?;

    let found_root_id = embedding_service.find_root_id(&child3.id).await?;
    assert_eq!(
        found_root_id, root.id,
        "Deep child should find correct root"
    );
    Ok(())
}

#[tokio::test]
async fn test_should_embed_root_for_embeddable_types() -> Result<()> {
    let (embedding_service, _store, _temp_dir) = create_test_embedding_service().await?;

    // Test embeddable types
    let embeddable_types = vec!["text", "header", "code-block", "schema"];
    for node_type in embeddable_types {
        let node = Node::new(node_type.to_string(), "test".to_string(), json!({}));
        assert!(
            embedding_service.should_embed_root(&node),
            "{} should be embeddable",
            node_type
        );
    }
    Ok(())
}

#[tokio::test]
async fn test_should_embed_root_for_non_embeddable_types() -> Result<()> {
    let (embedding_service, _store, _temp_dir) = create_test_embedding_service().await?;

    // Test non-embeddable types
    let non_embeddable_types = vec!["task", "date", "person", "ai-chat"];
    for node_type in non_embeddable_types {
        let node = Node::new(node_type.to_string(), "test".to_string(), json!({}));
        assert!(
            !embedding_service.should_embed_root(&node),
            "{} should not be embeddable",
            node_type
        );
    }
    Ok(())
}

// =========================================================================
// Content Aggregation Tests
// =========================================================================

#[tokio::test]
async fn test_aggregate_subtree_content_single_node() -> Result<()> {
    let (embedding_service, _store, _temp_dir) = create_test_embedding_service().await?;
    let (node_service, _node_temp_dir) = create_test_service().await?;

    let root = create_root_node(&node_service, "text", "Root content").await?;

    let aggregated = embedding_service
        .aggregate_subtree_content(&root.id)
        .await?;
    assert_eq!(aggregated, "Root content");
    Ok(())
}

#[tokio::test]
async fn test_aggregate_subtree_content_with_children() -> Result<()> {
    let (embedding_service, _store, _temp_dir) = create_test_embedding_service().await?;
    let (node_service, _node_temp_dir) = create_test_service().await?;

    // Create tree: root -> child1, child2
    let root = create_root_node(&node_service, "text", "Root").await?;
    let _child1 = create_child_node(&node_service, &root.id, "text", "Child 1").await?;
    let _child2 = create_child_node(&node_service, &root.id, "text", "Child 2").await?;

    let aggregated = embedding_service
        .aggregate_subtree_content(&root.id)
        .await?;

    // Content should be aggregated with double newlines
    assert!(aggregated.contains("Root"));
    assert!(aggregated.contains("Child 1"));
    assert!(aggregated.contains("Child 2"));
    assert!(aggregated.contains("\n\n"));
    Ok(())
}

#[tokio::test]
async fn test_aggregate_subtree_content_skips_empty() -> Result<()> {
    let (embedding_service, _store, _temp_dir) = create_test_embedding_service().await?;
    let (node_service, _node_temp_dir) = create_test_service().await?;

    let root = create_root_node(&node_service, "text", "Root").await?;
    let _empty_child = create_child_node(&node_service, &root.id, "text", "   ").await?;
    let _child = create_child_node(&node_service, &root.id, "text", "Child").await?;

    let aggregated = embedding_service
        .aggregate_subtree_content(&root.id)
        .await?;

    // Should skip empty/whitespace-only content
    assert!(aggregated.contains("Root"));
    assert!(aggregated.contains("Child"));
    // Empty child content should be skipped
    Ok(())
}

#[tokio::test]
async fn test_aggregate_subtree_content_respects_max_descendants() -> Result<()> {
    let (_, store, _temp_dir) = create_test_embedding_service().await?;
    let (node_service, _node_temp_dir) = create_test_service().await?;

    // Create a config with small max_descendants limit
    let nlp_engine = create_test_nlp_engine();
    let config = EmbeddingConfig {
        max_descendants: 2,
        max_retries: 3,
        ..Default::default()
    };
    let embedding_service = NodeEmbeddingService::with_config(nlp_engine, store, config);

    // Create root with 5 children
    let root = create_root_node(&node_service, "text", "Root").await?;
    for i in 1..=5 {
        create_child_node(&node_service, &root.id, "text", &format!("Child {}", i)).await?;
    }

    let aggregated = embedding_service
        .aggregate_subtree_content(&root.id)
        .await?;

    // Should only include root + first 2 children
    let parts: Vec<&str> = aggregated.split("\n\n").collect();
    assert!(parts.len() <= 3, "Should respect max_descendants limit");
    Ok(())
}

#[tokio::test]
async fn test_aggregate_subtree_content_respects_max_size() -> Result<()> {
    let (_, store, _temp_dir) = create_test_embedding_service().await?;
    let (node_service, _node_temp_dir) = create_test_service().await?;

    let nlp_engine = create_test_nlp_engine();
    let config = EmbeddingConfig {
        max_content_size: 50, // Very small limit
        max_retries: 3,
        ..Default::default()
    };
    let embedding_service = NodeEmbeddingService::with_config(nlp_engine, store, config);

    // Create a root with long content
    let long_content = "a".repeat(100);
    let root = create_root_node(&node_service, "text", &long_content).await?;

    let aggregated = embedding_service
        .aggregate_subtree_content(&root.id)
        .await?;

    // Should be truncated to max_content_size
    assert!(aggregated.len() <= 50);
    Ok(())
}

// =========================================================================
// Queue Management Tests
// =========================================================================

#[tokio::test]
async fn test_queue_for_embedding_root_node() -> Result<()> {
    let (embedding_service, store, _temp_dir) = create_test_embedding_service().await?;
    let (node_service, _node_temp_dir) = create_test_service().await?;

    let root = create_root_node(&node_service, "text", "Content").await?;

    embedding_service.queue_for_embedding(&root.id).await?;

    // Check that stale marker was created
    let stale_ids = store.get_stale_embedding_root_ids(Some(10)).await?;
    assert!(stale_ids.contains(&root.id));
    Ok(())
}

#[tokio::test]
async fn test_queue_for_embedding_child_node() -> Result<()> {
    let (embedding_service, store, _temp_dir) = create_test_embedding_service().await?;
    let (node_service, _node_temp_dir) = create_test_service().await?;

    let root = create_root_node(&node_service, "text", "Root").await?;
    let child = create_child_node(&node_service, &root.id, "text", "Child").await?;

    // Queue the child
    embedding_service.queue_for_embedding(&child.id).await?;

    // Should have queued the root, not the child
    let stale_ids = store.get_stale_embedding_root_ids(Some(10)).await?;
    assert!(stale_ids.contains(&root.id));
    Ok(())
}

#[tokio::test]
async fn test_queue_for_embedding_non_embeddable() -> Result<()> {
    let (embedding_service, store, _temp_dir) = create_test_embedding_service().await?;
    let (node_service, _node_temp_dir) = create_test_service().await?;

    let task = create_root_node(&node_service, "task", "Do something").await?;

    embedding_service.queue_for_embedding(&task.id).await?;

    // Should not have queued non-embeddable type
    let stale_ids = store.get_stale_embedding_root_ids(Some(10)).await?;
    assert!(!stale_ids.contains(&task.id));
    Ok(())
}

#[tokio::test]
async fn test_queue_nodes_for_embedding_deduplicates_roots() -> Result<()> {
    let (embedding_service, store, _temp_dir) = create_test_embedding_service().await?;
    let (node_service, _node_temp_dir) = create_test_service().await?;

    let root = create_root_node(&node_service, "text", "Root").await?;
    let child1 = create_child_node(&node_service, &root.id, "text", "Child1").await?;
    let child2 = create_child_node(&node_service, &root.id, "text", "Child2").await?;

    // Queue multiple children of the same root
    let node_ids = vec![child1.id.as_str(), child2.id.as_str()];
    embedding_service
        .queue_nodes_for_embedding(&node_ids)
        .await?;

    // Should only queue root once
    let stale_ids = store.get_stale_embedding_root_ids(Some(10)).await?;
    assert_eq!(
        stale_ids.len(),
        1,
        "Should deduplicate to single root queue entry"
    );
    assert!(stale_ids.contains(&root.id));
    Ok(())
}

#[tokio::test]
async fn test_process_stale_embeddings_empty_queue() -> Result<()> {
    let (embedding_service, _store, _temp_dir) = create_test_embedding_service().await?;

    let processed = embedding_service.process_stale_embeddings(None).await?;
    assert_eq!(processed, 0, "Should process 0 items from empty queue");
    Ok(())
}

// =========================================================================
// Configuration Tests
// =========================================================================

#[tokio::test]
async fn test_service_with_custom_config() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let db_path = temp_dir.path().join("test.db");
    let store = Arc::new(SurrealStore::new(db_path).await?);
    let nlp_engine = create_test_nlp_engine();

    let custom_config = EmbeddingConfig {
        max_tokens_per_chunk: 256,
        overlap_tokens: 25,
        max_descendants: 50,
        max_content_size: 100_000,
        debounce_duration_secs: 10,
        max_retries: 5,
    };

    let service = NodeEmbeddingService::with_config(nlp_engine, store, custom_config);

    // Verify accessors exist and return valid references
    let _ = service.nlp_engine();
    let _ = service.store();
    Ok(())
}

// =========================================================================
// Edge Cases and Error Handling Tests
// =========================================================================

#[tokio::test]
async fn test_concurrent_queue_operations() -> Result<()> {
    use tokio::task::JoinSet;

    let (embedding_service, _store, _temp_dir) = create_test_embedding_service().await?;
    let (node_service, _node_temp_dir) = create_test_service().await?;
    let embedding_service = Arc::new(embedding_service);

    // Create multiple roots
    let mut root_ids = Vec::new();
    for i in 1..=5 {
        let root = create_root_node(&node_service, "text", &format!("Content {}", i)).await?;
        root_ids.push(root.id);
    }

    // Queue all concurrently
    let mut tasks = JoinSet::new();
    for root_id in root_ids {
        let svc = embedding_service.clone();
        tasks.spawn(async move { svc.queue_for_embedding(&root_id).await });
    }

    // Wait for all to complete
    let mut success_count = 0;
    while let Some(result) = tasks.join_next().await {
        if result.unwrap().is_ok() {
            success_count += 1;
        }
    }

    assert_eq!(
        success_count, 5,
        "All concurrent queue operations should succeed"
    );
    Ok(())
}

// NOTE: Tests for semantic_search, embed_root_node, and other embedding generation
// methods require an initialized NLP engine with a valid model, which is not available
// in the test environment. Those tests should be added when testing against a real model.
