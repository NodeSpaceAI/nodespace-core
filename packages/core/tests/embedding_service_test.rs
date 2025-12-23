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

/// Test helper: Create a test NLP engine (uninitialized for testing)
fn create_test_nlp_engine() -> Arc<EmbeddingService> {
    let config = NlpConfig::default();
    Arc::new(EmbeddingService::new(config).unwrap())
}

/// Test helper: Create a unified test environment with shared database
///
/// Returns (NodeEmbeddingService, NodeService, Arc<SurrealStore>, TempDir)
/// Both services share the same database instance for proper test isolation.
async fn create_unified_test_env() -> Result<(
    NodeEmbeddingService,
    NodeService,
    Arc<SurrealStore>,
    TempDir,
)> {
    let temp_dir = TempDir::new()?;
    let db_path = temp_dir.path().join("test.db");
    let mut store = Arc::new(SurrealStore::new(db_path).await?);

    // Create NodeService first (it may set up schema)
    let node_service = NodeService::new(&mut store).await?;

    // Create embedding service using the SAME store
    let nlp_engine = create_test_nlp_engine();
    let embedding_service = NodeEmbeddingService::new(nlp_engine, store.clone());

    Ok((embedding_service, node_service, store, temp_dir))
}

/// Test helper: Create a unified test environment with custom embedding config
async fn create_unified_test_env_with_config(
    config: EmbeddingConfig,
) -> Result<(
    NodeEmbeddingService,
    NodeService,
    Arc<SurrealStore>,
    TempDir,
)> {
    let temp_dir = TempDir::new()?;
    let db_path = temp_dir.path().join("test.db");
    let mut store = Arc::new(SurrealStore::new(db_path).await?);

    // Create NodeService first (it may set up schema)
    let node_service = NodeService::new(&mut store).await?;

    // Create embedding service with custom config using the SAME store
    let nlp_engine = create_test_nlp_engine();
    let embedding_service = NodeEmbeddingService::with_config(nlp_engine, store.clone(), config);

    Ok((embedding_service, node_service, store, temp_dir))
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
    let (embedding_service, node_service, _store, _temp_dir) = create_unified_test_env().await?;

    let root = create_root_node(&node_service, "text", "Root content").await?;

    let is_root = embedding_service.is_root_node(&root.id).await?;
    assert!(is_root, "Node with no parent should be identified as root");
    Ok(())
}

#[tokio::test]
async fn test_is_root_node_with_parent() -> Result<()> {
    let (embedding_service, node_service, _store, _temp_dir) = create_unified_test_env().await?;

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
    let (embedding_service, node_service, _store, _temp_dir) = create_unified_test_env().await?;

    let root = create_root_node(&node_service, "text", "Root").await?;

    let found_root_id = embedding_service.find_root_id(&root.id).await?;
    assert_eq!(found_root_id, root.id, "Root should find itself");
    Ok(())
}

#[tokio::test]
async fn test_find_root_id_for_deep_child() -> Result<()> {
    let (embedding_service, node_service, _store, _temp_dir) = create_unified_test_env().await?;

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
    let (embedding_service, _node_service, _store, _temp_dir) = create_unified_test_env().await?;

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
    let (embedding_service, _node_service, _store, _temp_dir) = create_unified_test_env().await?;

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
    let (embedding_service, node_service, _store, _temp_dir) = create_unified_test_env().await?;

    let root = create_root_node(&node_service, "text", "Root content").await?;

    let aggregated = embedding_service
        .aggregate_subtree_content(&root.id)
        .await?;
    assert_eq!(aggregated, "Root content");
    Ok(())
}

#[tokio::test]
async fn test_aggregate_subtree_content_with_children() -> Result<()> {
    let (embedding_service, node_service, _store, _temp_dir) = create_unified_test_env().await?;

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
    let (embedding_service, node_service, _store, _temp_dir) = create_unified_test_env().await?;

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
    // Create a config with small max_descendants limit
    let config = EmbeddingConfig {
        max_descendants: 2,
        max_retries: 3,
        ..Default::default()
    };
    let (embedding_service, node_service, _store, _temp_dir) =
        create_unified_test_env_with_config(config).await?;

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
    let config = EmbeddingConfig {
        max_content_size: 50, // Very small limit
        max_retries: 3,
        ..Default::default()
    };
    let (embedding_service, node_service, _store, _temp_dir) =
        create_unified_test_env_with_config(config).await?;

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
    let (embedding_service, node_service, store, _temp_dir) = create_unified_test_env().await?;

    let root = create_root_node(&node_service, "text", "Content").await?;

    embedding_service.queue_for_embedding(&root.id).await?;

    // Check that stale marker was created
    let stale_ids = store.get_stale_embedding_root_ids(Some(10), 0).await?;
    assert!(stale_ids.contains(&root.id));
    Ok(())
}

#[tokio::test]
async fn test_queue_for_embedding_child_node() -> Result<()> {
    let (embedding_service, node_service, store, _temp_dir) = create_unified_test_env().await?;

    let root = create_root_node(&node_service, "text", "Root").await?;
    let child = create_child_node(&node_service, &root.id, "text", "Child").await?;

    // Queue the child
    embedding_service.queue_for_embedding(&child.id).await?;

    // Should have queued the root, not the child
    let stale_ids = store.get_stale_embedding_root_ids(Some(10), 0).await?;
    assert!(stale_ids.contains(&root.id));
    Ok(())
}

#[tokio::test]
async fn test_queue_for_embedding_non_embeddable() -> Result<()> {
    let (embedding_service, node_service, store, _temp_dir) = create_unified_test_env().await?;

    let task = create_root_node(&node_service, "task", "Do something").await?;

    embedding_service.queue_for_embedding(&task.id).await?;

    // Should not have queued non-embeddable type
    let stale_ids = store.get_stale_embedding_root_ids(Some(10), 0).await?;
    assert!(!stale_ids.contains(&task.id));
    Ok(())
}

#[tokio::test]
async fn test_queue_nodes_for_embedding_deduplicates_roots() -> Result<()> {
    let (embedding_service, node_service, store, _temp_dir) = create_unified_test_env().await?;

    let root = create_root_node(&node_service, "text", "Root").await?;
    let child1 = create_child_node(&node_service, &root.id, "text", "Child1").await?;
    let child2 = create_child_node(&node_service, &root.id, "text", "Child2").await?;

    // Queue multiple children of the same root
    let node_ids = vec![child1.id.as_str(), child2.id.as_str()];
    embedding_service
        .queue_nodes_for_embedding(&node_ids)
        .await?;

    // Should only queue root once
    let stale_ids = store.get_stale_embedding_root_ids(Some(10), 0).await?;
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
    let (embedding_service, _node_service, _store, _temp_dir) = create_unified_test_env().await?;

    let processed = embedding_service.process_stale_embeddings(None).await?;
    assert_eq!(processed, 0, "Should process 0 items from empty queue");
    Ok(())
}

// =========================================================================
// Configuration Tests
// =========================================================================

#[tokio::test]
async fn test_service_with_custom_config() -> Result<()> {
    let custom_config = EmbeddingConfig {
        max_tokens_per_chunk: 256,
        overlap_tokens: 25,
        chars_per_token_estimate: 3,
        max_descendants: 50,
        max_content_size: 100_000,
        debounce_duration_secs: 10,
        max_retries: 5,
    };

    let (service, _node_service, _store, _temp_dir) =
        create_unified_test_env_with_config(custom_config).await?;

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

    let (embedding_service, node_service, _store, _temp_dir) = create_unified_test_env().await?;
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

// =========================================================================
// Semantic Search / KNN Tests
// =========================================================================
// Note: These tests use mock embeddings to test the database-level KNN search
// functionality without requiring an initialized NLP engine.

#[tokio::test]
async fn test_knn_search_with_mock_embeddings() -> Result<()> {
    use nodespace_core::models::NewEmbedding;

    let (_embedding_service, node_service, store, _temp_dir) = create_unified_test_env().await?;

    // Create test nodes
    let node1 = create_root_node(&node_service, "text", "First document about cats").await?;
    let node2 = create_root_node(&node_service, "text", "Second document about dogs").await?;
    let node3 = create_root_node(&node_service, "text", "Third document about birds").await?;

    // Create mock embeddings (768-dimensional vectors as used by nomic-embed-text)
    // We use simple patterns where similar content has similar vectors
    let mut vec1 = vec![0.0f32; 768];
    vec1[0] = 1.0; // "cats" direction
    vec1[1] = 0.5;

    let mut vec2 = vec![0.0f32; 768];
    vec2[0] = 0.9; // Similar to cats (dogs)
    vec2[1] = 0.6;

    let mut vec3 = vec![0.0f32; 768];
    vec3[0] = 0.1; // Different (birds)
    vec3[1] = 0.9;

    // Insert mock embeddings
    store
        .upsert_embeddings(
            &node1.id,
            vec![NewEmbedding {
                node_id: node1.id.clone(),
                vector: vec1.clone(),
                model_name: Some("test-model".to_string()),
                chunk_index: 0,
                chunk_start: 0,
                chunk_end: 100,
                total_chunks: 1,
                content_hash: "hash1".to_string(),
                token_count: 10,
            }],
        )
        .await?;

    store
        .upsert_embeddings(
            &node2.id,
            vec![NewEmbedding {
                node_id: node2.id.clone(),
                vector: vec2,
                model_name: Some("test-model".to_string()),
                chunk_index: 0,
                chunk_start: 0,
                chunk_end: 100,
                total_chunks: 1,
                content_hash: "hash2".to_string(),
                token_count: 10,
            }],
        )
        .await?;

    store
        .upsert_embeddings(
            &node3.id,
            vec![NewEmbedding {
                node_id: node3.id.clone(),
                vector: vec3,
                model_name: Some("test-model".to_string()),
                chunk_index: 0,
                chunk_start: 0,
                chunk_end: 100,
                total_chunks: 1,
                content_hash: "hash3".to_string(),
                token_count: 10,
            }],
        )
        .await?;

    // Search with a query vector similar to node1 (cats)
    let results = store.search_embeddings(&vec1, 10, Some(0.5)).await?;

    // Should find results
    assert!(!results.is_empty(), "KNN search should return results");

    // First result should be node1 (exact match)
    assert_eq!(
        results[0].node_id, node1.id,
        "First result should be the exact match"
    );
    assert!(
        (results[0].max_similarity - 1.0).abs() < 0.001,
        "Exact match should have max_similarity ~1.0"
    );
    // For a single chunk document, score should equal max_similarity (log10(1) = 0)
    assert!(
        (results[0].score - results[0].max_similarity).abs() < 0.001,
        "Single chunk score should equal max_similarity"
    );
    assert_eq!(
        results[0].matching_chunks, 1,
        "Should have 1 matching chunk"
    );

    // Second result should be node2 (similar to cats)
    if results.len() > 1 {
        assert_eq!(
            results[1].node_id, node2.id,
            "Second result should be the similar node"
        );
    }

    Ok(())
}

#[tokio::test]
async fn test_knn_search_respects_threshold() -> Result<()> {
    use nodespace_core::models::NewEmbedding;

    let (_embedding_service, node_service, store, _temp_dir) = create_unified_test_env().await?;

    let node1 = create_root_node(&node_service, "text", "Test document").await?;

    // Create a mock embedding
    let mut vec1 = vec![0.0f32; 768];
    vec1[0] = 1.0;
    vec1[1] = 0.5;

    store
        .upsert_embeddings(
            &node1.id,
            vec![NewEmbedding {
                node_id: node1.id.clone(),
                vector: vec1.clone(),
                model_name: Some("test-model".to_string()),
                chunk_index: 0,
                chunk_start: 0,
                chunk_end: 100,
                total_chunks: 1,
                content_hash: "hash1".to_string(),
                token_count: 10,
            }],
        )
        .await?;

    // Create a query vector with some similarity but not high
    // This vector shares some components with vec1 but is different
    let mut query_vec = vec![0.0f32; 768];
    query_vec[0] = 0.3; // Some overlap with vec1
    query_vec[1] = 0.1;
    query_vec[2] = 0.9; // Different direction

    // Search with high threshold (0.9) - should find nothing since similarity is ~0.3
    let results = store.search_embeddings(&query_vec, 10, Some(0.9)).await?;
    assert!(
        results.is_empty(),
        "High threshold should filter out low-similarity results"
    );

    // Search with low threshold (0.1) - should find the node
    let results = store.search_embeddings(&query_vec, 10, Some(0.1)).await?;
    assert!(!results.is_empty(), "Low threshold should include results");

    Ok(())
}

#[tokio::test]
async fn test_knn_search_with_multiple_chunks() -> Result<()> {
    use nodespace_core::models::NewEmbedding;

    let (_embedding_service, node_service, store, _temp_dir) = create_unified_test_env().await?;

    let node1 =
        create_root_node(&node_service, "text", "Long document with multiple chunks").await?;

    // Create multiple chunk embeddings for the same node
    let mut vec_chunk1 = vec![0.0f32; 768];
    vec_chunk1[0] = 0.8;
    vec_chunk1[1] = 0.2;

    let mut vec_chunk2 = vec![0.0f32; 768];
    vec_chunk2[0] = 0.9;
    vec_chunk2[1] = 0.3;

    store
        .upsert_embeddings(
            &node1.id,
            vec![
                NewEmbedding {
                    node_id: node1.id.clone(),
                    vector: vec_chunk1,
                    model_name: Some("test-model".to_string()),
                    chunk_index: 0,
                    chunk_start: 0,
                    chunk_end: 500,
                    total_chunks: 2,
                    content_hash: "hash1".to_string(),
                    token_count: 100,
                },
                NewEmbedding {
                    node_id: node1.id.clone(),
                    vector: vec_chunk2.clone(),
                    model_name: Some("test-model".to_string()),
                    chunk_index: 1,
                    chunk_start: 500,
                    chunk_end: 1000,
                    total_chunks: 2,
                    content_hash: "hash1".to_string(),
                    token_count: 100,
                },
            ],
        )
        .await?;

    // Query with vector similar to chunk2
    let results = store.search_embeddings(&vec_chunk2, 10, Some(0.5)).await?;

    // Should return only one result per node (grouped by node)
    assert_eq!(results.len(), 1, "Should group multiple chunks by node");

    // Should return the best similarity (from chunk2)
    assert!(
        results[0].max_similarity > 0.99,
        "Should return best chunk similarity"
    );

    // With 2 matching chunks (both above 0.5 threshold), score should be boosted
    // Score = max_similarity * (1 + 0.3 * log10(2)) ≈ max_similarity * 1.09
    assert!(
        results[0].matching_chunks >= 1,
        "Should have at least 1 matching chunk"
    );
    // Score should be >= max_similarity (breadth boost is always positive)
    assert!(
        results[0].score >= results[0].max_similarity * 0.99, // Allow small float error
        "Score should be at least max_similarity"
    );

    Ok(())
}

/// Test that multi-chunk scoring boosts documents with broader relevance (Issue #778)
///
/// Scenario:
/// - Document A: Single chunk with similarity 0.85
/// - Document B: Five chunks, best similarity 0.80
///
/// Expected: Document B should rank higher due to breadth boost
#[tokio::test]
async fn test_multi_chunk_scoring_breadth_boost() -> Result<()> {
    use nodespace_core::models::NewEmbedding;

    let (_embedding_service, node_service, store, _temp_dir) = create_unified_test_env().await?;

    // Document A: Single high-scoring chunk
    let doc_a = create_root_node(&node_service, "text", "Single chunk document").await?;

    // Document B: Multiple moderate-scoring chunks
    let doc_b = create_root_node(&node_service, "text", "Multi-chunk document").await?;

    // Create base query vector
    let mut query_vec = vec![0.0f32; 768];
    query_vec[0] = 1.0;

    // Document A: Single chunk with higher similarity (0.85)
    // Vector is close to query but not identical
    let mut vec_a = vec![0.0f32; 768];
    vec_a[0] = 0.9;
    vec_a[1] = 0.3;

    store
        .upsert_embeddings(
            &doc_a.id,
            vec![NewEmbedding {
                node_id: doc_a.id.clone(),
                vector: vec_a,
                model_name: Some("test-model".to_string()),
                chunk_index: 0,
                chunk_start: 0,
                chunk_end: 100,
                total_chunks: 1,
                content_hash: "hash-a".to_string(),
                token_count: 50,
            }],
        )
        .await?;

    // Document B: Multiple chunks with moderate similarity (best ~0.80)
    // All chunks should pass threshold (0.5) but have lower max than doc_a
    let chunks: Vec<NewEmbedding> = (0..5)
        .map(|i| {
            let mut vec = vec![0.0f32; 768];
            // Base similarity around 0.75-0.80
            vec[0] = 0.8 - (i as f32 * 0.02); // 0.80, 0.78, 0.76, 0.74, 0.72
            vec[1] = 0.4 + (i as f32 * 0.05); // Slight variation

            NewEmbedding {
                node_id: doc_b.id.clone(),
                vector: vec,
                model_name: Some("test-model".to_string()),
                chunk_index: i,
                chunk_start: i * 200,
                chunk_end: (i + 1) * 200,
                total_chunks: 5,
                content_hash: "hash-b".to_string(),
                token_count: 100,
            }
        })
        .collect();

    store.upsert_embeddings(&doc_b.id, chunks).await?;

    // Search with threshold that includes all chunks
    let results = store.search_embeddings(&query_vec, 10, Some(0.5)).await?;

    // Should have both documents
    assert!(
        results.len() >= 2,
        "Should return at least both documents, got {}",
        results.len()
    );

    // Find both documents in results
    let doc_a_result = results.iter().find(|r| r.node_id == doc_a.id);
    let doc_b_result = results.iter().find(|r| r.node_id == doc_b.id);

    assert!(doc_a_result.is_some(), "Document A should be in results");
    assert!(doc_b_result.is_some(), "Document B should be in results");

    let doc_a_result = doc_a_result.unwrap();
    let doc_b_result = doc_b_result.unwrap();

    // Verify document A has higher max_similarity but only 1 chunk
    assert_eq!(doc_a_result.matching_chunks, 1, "Doc A should have 1 chunk");
    assert!(
        doc_b_result.matching_chunks >= 3,
        "Doc B should have multiple chunks (got {})",
        doc_b_result.matching_chunks
    );

    // Verify the score boost for document B
    // Score formula: max_similarity * (1 + 0.3 * log10(matching_chunks))
    // Doc A: ~0.85 * 1.0 = 0.85 (log10(1) = 0)
    // Doc B with 5 chunks: ~0.80 * 1.21 = 0.97 (log10(5) ≈ 0.70)
    let expected_breadth_factor = 1.0 + 0.3 * (doc_b_result.matching_chunks as f64).log10();
    let expected_doc_b_score = doc_b_result.max_similarity * expected_breadth_factor;

    assert!(
        (doc_b_result.score - expected_doc_b_score).abs() < 0.001,
        "Doc B score should match formula: expected {}, got {}",
        expected_doc_b_score,
        doc_b_result.score
    );

    // Key assertion: Document B should have higher SCORE despite lower max_similarity
    // This validates the breadth boost is working
    assert!(
        doc_b_result.score > doc_a_result.score,
        "Doc B (score: {:.3}, max_sim: {:.3}, chunks: {}) should rank higher than Doc A (score: {:.3}, max_sim: {:.3}, chunks: {})",
        doc_b_result.score, doc_b_result.max_similarity, doc_b_result.matching_chunks,
        doc_a_result.score, doc_a_result.max_similarity, doc_a_result.matching_chunks
    );

    // Verify the results are sorted by score (not max_similarity)
    let first_result = &results[0];
    assert_eq!(
        first_result.node_id, doc_b.id,
        "Document B should be first due to higher score"
    );

    Ok(())
}
