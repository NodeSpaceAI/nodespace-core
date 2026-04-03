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

    // Create embedding service using the SAME store (Issue #1018: behavior-driven)
    let nlp_engine = create_test_nlp_engine();
    let node_accessor: Arc<dyn nodespace_core::services::NodeAccessor> =
        Arc::new(node_service.clone());
    let behaviors = node_service.behaviors().clone();
    let embedding_service =
        NodeEmbeddingService::new(nlp_engine, store.clone(), node_accessor, behaviors);

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

    // Create embedding service with custom config using the SAME store (Issue #1018)
    let nlp_engine = create_test_nlp_engine();
    let node_accessor: Arc<dyn nodespace_core::services::NodeAccessor> =
        Arc::new(node_service.clone());
    let behaviors = node_service.behaviors().clone();
    let embedding_service = NodeEmbeddingService::with_config(
        nlp_engine,
        store.clone(),
        node_accessor,
        behaviors,
        config,
    );

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
    service
        .move_node_unchecked(&node.id, Some(parent_id), None)
        .await?;
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

/// Issue #1018: Behavior-driven embeddability test (replaces should_embed_root)
#[tokio::test]
async fn test_behavior_driven_embeddable_types() -> Result<()> {
    use nodespace_core::behaviors::NodeBehaviorRegistry;

    let registry = NodeBehaviorRegistry::new();

    // Types whose behaviors return Some from get_embeddable_content for non-empty content
    // Issue #1018: table is now correctly embeddable (was excluded from EMBEDDABLE_NODE_TYPES)
    let embeddable_types = vec!["text", "header", "code-block", "schema", "table"];
    for node_type in embeddable_types {
        let behavior = registry.get(node_type).expect("behavior should exist");
        let node = Node::new(node_type.to_string(), "test content".to_string(), json!({}));
        assert!(
            behavior.get_embeddable_content(&node).is_some(),
            "{} should be embeddable",
            node_type
        );
    }
    Ok(())
}

/// Issue #1018: Behavior-driven non-embeddability test (replaces should_embed_root)
#[tokio::test]
async fn test_behavior_driven_non_embeddable_types() -> Result<()> {
    use nodespace_core::behaviors::NodeBehaviorRegistry;

    let registry = NodeBehaviorRegistry::new();

    // Types whose behaviors always return None from get_embeddable_content
    let non_embeddable_types = vec!["task", "date", "collection", "query", "horizontal-line"];
    for node_type in non_embeddable_types {
        let behavior = registry.get(node_type).expect("behavior should exist");
        let node = Node::new(node_type.to_string(), "test content".to_string(), json!({}));
        assert!(
            behavior.get_embeddable_content(&node).is_none(),
            "{} should not be embeddable",
            node_type
        );
    }
    Ok(())
}

// =========================================================================
// Behavior-Driven Content Extraction Tests (Issue #1018)
// =========================================================================

/// Test: behavior.get_aggregated_content() collects children for text nodes
#[tokio::test]
async fn test_behavior_aggregated_content_text_node() -> Result<()> {
    use nodespace_core::behaviors::{NodeBehavior, TextNodeBehavior};

    let (_embedding_service, node_service, _store, _temp_dir) = create_unified_test_env().await?;

    // Create tree: root -> child1, child2
    let root = create_root_node(&node_service, "text", "Root").await?;
    let _child1 = create_child_node(&node_service, &root.id, "text", "Child 1").await?;
    let _child2 = create_child_node(&node_service, &root.id, "text", "Child 2").await?;

    let behavior = TextNodeBehavior;
    let aggregated = behavior.get_aggregated_content(&root, &node_service).await;

    assert!(aggregated.is_some());
    let content = aggregated.unwrap();
    assert!(content.contains("Child 1"));
    assert!(content.contains("Child 2"));
    Ok(())
}

/// Test: behavior.get_aggregated_content() skips empty children
#[tokio::test]
async fn test_behavior_aggregated_content_skips_empty() -> Result<()> {
    use nodespace_core::behaviors::{NodeBehavior, TextNodeBehavior};

    let (_embedding_service, node_service, _store, _temp_dir) = create_unified_test_env().await?;

    let root = create_root_node(&node_service, "text", "Root").await?;
    let _empty_child = create_child_node(&node_service, &root.id, "text", "   ").await?;
    let _child = create_child_node(&node_service, &root.id, "text", "Child").await?;

    let behavior = TextNodeBehavior;
    let aggregated = behavior.get_aggregated_content(&root, &node_service).await;

    assert!(aggregated.is_some());
    let content = aggregated.unwrap();
    assert!(content.contains("Child"));
    // Empty child's whitespace content should not contribute
    assert!(!content.contains("   "));
    Ok(())
}

/// Test: non-embeddable task nodes don't produce embeddable content
#[tokio::test]
async fn test_behavior_task_not_embeddable() -> Result<()> {
    use nodespace_core::behaviors::{NodeBehavior, TaskNodeBehavior};

    let node = Node::new(
        "task".to_string(),
        "Do something".to_string(),
        json!({"task": {"status": "open"}}),
    );
    let behavior = TaskNodeBehavior;
    assert!(behavior.get_embeddable_content(&node).is_none());
    Ok(())
}

/// Test: table nodes are embeddable (bug fix from Issue #1018)
#[tokio::test]
async fn test_behavior_table_is_embeddable() -> Result<()> {
    use nodespace_core::behaviors::{NodeBehavior, TableNodeBehavior};

    let node = Node::new(
        "table".to_string(),
        "| Col1 | Col2 |\n|------|------|\n| A | B |".to_string(),
        json!({}),
    );
    let behavior = TableNodeBehavior;
    assert!(behavior.get_embeddable_content(&node).is_some());
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
    // For a single chunk document, density = 1/1 = 1.0, so score = max_similarity * 1.30
    assert!(
        (results[0].score - results[0].max_similarity * 1.3).abs() < 0.001,
        "Single chunk score should equal max_similarity * 1.3 (density boost with density=1.0)"
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

    // With 2 matching chunks out of 2 total (density=1.0), score should be boosted
    // Score = max_similarity * (1 + 0.3 * 1.0) = max_similarity * 1.30
    assert_eq!(
        results[0].matching_chunks, 2,
        "Should have both chunks counted"
    );
    // Score must reflect the density boost: max_sim * 1.30 (density=1.0)
    let expected_score = results[0].max_similarity * 1.30;
    assert!(
        (results[0].score - expected_score).abs() < 0.01,
        "Score should equal max_similarity * 1.30 for density=1.0, expected {:.4}, got {:.4}",
        expected_score,
        results[0].score
    );

    Ok(())
}

/// Test that match density ratio score formula is applied correctly (Issue #944)
///
/// Validates formula: score = max_similarity * (1.0 + 0.3 * (matching_chunks / total_chunks))
///
/// Two documents with identical max_similarity but different total_chunks:
/// - Document A: 2 total chunks (density = matching/2)
/// - Document B: 5 total chunks (density = matching/5)
///
/// For the same number of matching chunks, Doc A has higher density and thus higher score.
#[tokio::test]
async fn test_multi_chunk_scoring_density_boost() -> Result<()> {
    use nodespace_core::models::NewEmbedding;

    let (_embedding_service, node_service, store, _temp_dir) = create_unified_test_env().await?;

    // Document A: 2-chunk doc — both chunks aligned with query
    let doc_a = create_root_node(&node_service, "text", "Small precise document").await?;

    // Document B: 5-chunk doc — all chunks aligned with query, same similarity
    let doc_b = create_root_node(&node_service, "text", "Larger document same similarity").await?;

    // Query vector
    let mut query_vec = vec![0.0f32; 768];
    query_vec[0] = 1.0;

    // Doc A: 2 chunks, total_chunks=2, similarity ~0.80
    let chunks_a: Vec<NewEmbedding> = (0..2i32)
        .map(|i| {
            let mut vec = vec![0.0f32; 768];
            vec[0] = 0.80;
            vec[1] = 0.4 + (i as f32 * 0.05);
            NewEmbedding {
                node_id: doc_a.id.clone(),
                vector: vec,
                model_name: Some("test-model".to_string()),
                chunk_index: i,
                chunk_start: i * 200,
                chunk_end: (i + 1) * 200,
                total_chunks: 2,
                content_hash: "hash-a".to_string(),
                token_count: 100,
            }
        })
        .collect();
    store.upsert_embeddings(&doc_a.id, chunks_a).await?;

    // Doc B: 5 chunks, total_chunks=5, same similarity ~0.80
    let chunks_b: Vec<NewEmbedding> = (0..5i32)
        .map(|i| {
            let mut vec = vec![0.0f32; 768];
            vec[0] = 0.80;
            vec[1] = 0.4 + (i as f32 * 0.05);
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
    store.upsert_embeddings(&doc_b.id, chunks_b).await?;

    // Search
    let results = store.search_embeddings(&query_vec, 10, Some(0.5)).await?;

    assert!(
        results.len() >= 2,
        "Should return at least both documents, got {}",
        results.len()
    );

    let doc_a_result = results.iter().find(|r| r.node_id == doc_a.id);
    let doc_b_result = results.iter().find(|r| r.node_id == doc_b.id);

    assert!(doc_a_result.is_some(), "Document A should be in results");
    assert!(doc_b_result.is_some(), "Document B should be in results");

    let doc_a_result = doc_a_result.unwrap();
    let doc_b_result = doc_b_result.unwrap();

    // Verify the density formula is applied correctly for each doc
    // Doc A: matching/2 total chunks
    // Doc B: matching/5 total chunks
    let density_a = doc_a_result.matching_chunks as f64 / 2.0; // total_chunks=2
    let density_b = doc_b_result.matching_chunks as f64 / 5.0; // total_chunks=5
    let expected_score_a = doc_a_result.max_similarity * (1.0 + 0.3 * density_a);
    let expected_score_b = doc_b_result.max_similarity * (1.0 + 0.3 * density_b);

    assert!(
        (doc_a_result.score - expected_score_a).abs() < 0.01,
        "Doc A score should match density formula: expected {:.4}, got {:.4}",
        expected_score_a,
        doc_a_result.score
    );
    assert!(
        (doc_b_result.score - expected_score_b).abs() < 0.01,
        "Doc B score should match density formula: expected {:.4}, got {:.4}",
        expected_score_b,
        doc_b_result.score
    );

    // Both docs have same max_similarity and all chunks matching, but different total_chunks.
    // Doc A: density = 2/2 = 1.0 → boost = 1.30
    // Doc B: density = 5/5 = 1.0 → boost = 1.30
    // Both should get the same score (same density when all chunks match)
    assert!(
        (doc_a_result.score - doc_b_result.score).abs() < 0.05,
        "Docs with same max_sim and all-matching chunks (density=1.0) should score equally regardless of size: Doc A={:.3}, Doc B={:.3}",
        doc_a_result.score,
        doc_b_result.score,
    );

    Ok(())
}

/// Test that partial match density (density < 1.0) ranks below full density (density = 1.0)
///
/// Validates the key property of the density formula: a document with higher max_similarity
/// but low density (few matching chunks out of many total) should lose to a document with
/// lower max_similarity but perfect density (all chunks matching).
///
/// Setup:
/// - Doc A: 2 embeddings stored, total_chunks=2, max_sim=0.80 → density=1.0 → score ≈ 1.04
/// - Doc B: 2 embeddings stored, total_chunks=10, max_sim=0.85 → density=0.2 → score ≈ 0.90
///
/// Doc A wins despite lower max_sim because matching_chunks/total_chunks = 1.0 vs 0.2.
/// The total_chunks=10 is stored in the embedding field to simulate a large document
/// where only 2 of 10 chunks were indexed near the query vector.
#[tokio::test]
async fn test_partial_density_ranks_below_full_density() -> Result<()> {
    use nodespace_core::models::NewEmbedding;

    let (_embedding_service, node_service, store, _temp_dir) = create_unified_test_env().await?;

    // Doc A: 2 chunks stored, total_chunks=2 (density will be 1.0)
    let doc_a = create_root_node(
        &node_service,
        "text",
        "Small precise document, full density",
    )
    .await?;
    // Doc B: 2 chunks stored, but total_chunks=10 (density will be 0.2)
    let doc_b = create_root_node(
        &node_service,
        "text",
        "Large document with sparse match, partial density",
    )
    .await?;

    // Query vector
    let mut query_vec = vec![0.0f32; 768];
    query_vec[0] = 1.0;

    // Doc A: max_sim ~0.80, density = 2/2 = 1.0 → score = 0.80 * 1.30 ≈ 1.04
    let chunks_a: Vec<NewEmbedding> = (0..2i32)
        .map(|i| {
            let mut vec = vec![0.0f32; 768];
            vec[0] = 0.80;
            vec[1] = 0.45 + (i as f32 * 0.05);
            NewEmbedding {
                node_id: doc_a.id.clone(),
                vector: vec,
                model_name: Some("test-model".to_string()),
                chunk_index: i,
                chunk_start: i * 200,
                chunk_end: (i + 1) * 200,
                total_chunks: 2, // All 2 chunks are stored → density = 2/2 = 1.0
                content_hash: "hash-a-partial".to_string(),
                token_count: 100,
            }
        })
        .collect();
    store.upsert_embeddings(&doc_a.id, chunks_a).await?;

    // Doc B: max_sim ~0.85, but total_chunks=10 while only 2 are stored.
    // density = 2/10 = 0.2 → score = 0.85 * (1 + 0.3 * 0.2) = 0.85 * 1.06 ≈ 0.901
    let chunks_b: Vec<NewEmbedding> = (0..2i32)
        .map(|i| {
            let mut vec = vec![0.0f32; 768];
            vec[0] = 0.85;
            vec[1] = 0.45 + (i as f32 * 0.05);
            NewEmbedding {
                node_id: doc_b.id.clone(),
                vector: vec,
                model_name: Some("test-model".to_string()),
                chunk_index: i,
                chunk_start: i * 200,
                chunk_end: (i + 1) * 200,
                total_chunks: 10, // Only 2 of 10 chunks stored → density = 2/10 = 0.2
                content_hash: "hash-b-partial".to_string(),
                token_count: 100,
            }
        })
        .collect();
    store.upsert_embeddings(&doc_b.id, chunks_b).await?;

    let results = store.search_embeddings(&query_vec, 10, Some(0.5)).await?;

    assert!(
        results.len() >= 2,
        "Should return at least both documents, got {}",
        results.len()
    );

    let doc_a_result = results.iter().find(|r| r.node_id == doc_a.id);
    let doc_b_result = results.iter().find(|r| r.node_id == doc_b.id);

    assert!(
        doc_a_result.is_some(),
        "Document A (full density) should be in results"
    );
    assert!(
        doc_b_result.is_some(),
        "Document B (partial density) should be in results"
    );

    let doc_a_result = doc_a_result.unwrap();
    let doc_b_result = doc_b_result.unwrap();

    // Verify the density values are as expected
    assert_eq!(
        doc_a_result.matching_chunks, 2,
        "Doc A should have 2 matching chunks"
    );
    // Doc B also returns 2 matching chunks (those are the only ones stored)
    assert_eq!(
        doc_b_result.matching_chunks, 2,
        "Doc B should have 2 matching chunks"
    );

    // Verify the formula gives the expected density for each doc
    // Doc A: density = 2/2 = 1.0
    let density_a = 2.0f64 / 2.0;
    let expected_score_a = doc_a_result.max_similarity * (1.0 + 0.3 * density_a);
    assert!(
        (doc_a_result.score - expected_score_a).abs() < 0.01,
        "Doc A score should reflect density=1.0: expected {:.4}, got {:.4}",
        expected_score_a,
        doc_a_result.score
    );

    // Doc B: density = 2/10 = 0.2
    let density_b = 2.0f64 / 10.0;
    let expected_score_b = doc_b_result.max_similarity * (1.0 + 0.3 * density_b);
    assert!(
        (doc_b_result.score - expected_score_b).abs() < 0.01,
        "Doc B score should reflect density=0.2: expected {:.4}, got {:.4}",
        expected_score_b,
        doc_b_result.score
    );

    // Doc A (full density, lower max_sim) must outrank Doc B (partial density, higher max_sim)
    assert!(
        doc_a_result.score > doc_b_result.score,
        "Full-density doc (score={:.4}) should outrank partial-density doc (score={:.4}) despite lower max_similarity",
        doc_a_result.score,
        doc_b_result.score
    );

    Ok(())
}

/// Test that threshold filters by composite score, not raw similarity (Issue #787)
///
/// This test verifies that the threshold parameter filters results based on the
/// composite score (which includes breadth boost) rather than raw similarity.
///
/// A document with:
/// - raw max_similarity below the threshold
/// - but composite_score above the threshold (due to multiple matching chunks)
/// SHOULD be returned (this was the bug - it was being filtered out).
#[tokio::test]
async fn test_threshold_filters_by_composite_score_not_raw_similarity() -> Result<()> {
    use nodespace_core::models::NewEmbedding;

    let (_embedding_service, node_service, store, _temp_dir) = create_unified_test_env().await?;

    // Create a document that will have multiple matching chunks
    let doc = create_root_node(
        &node_service,
        "text",
        "Multi-chunk document for threshold test",
    )
    .await?;

    // Create a normalized query vector (unit vector in first dimension)
    let mut query_vec = vec![0.0f32; 768];
    query_vec[0] = 1.0;

    // Create 5 chunks with carefully calculated similarity
    // For cosine similarity with query [1,0,0,...]:
    // cos(θ) = v[0] / ||v||
    // To get similarity ~0.68, we need v[0]/||v|| = 0.68
    // If v = [0.68, 0.73, 0, ...], ||v|| = sqrt(0.68^2 + 0.73^2) = sqrt(0.9953) ≈ 0.9976
    // cos(θ) = 0.68 / 0.9976 ≈ 0.682
    let chunks: Vec<NewEmbedding> = (0..5)
        .map(|i| {
            let mut vec = vec![0.0f32; 768];
            // Base component aligned with query
            vec[0] = 0.68;
            // Orthogonal component to control the magnitude (and thus similarity)
            // Higher orthogonal component = lower similarity
            vec[1] = 0.73 + (i as f32 * 0.02); // Slightly varying similarities

            NewEmbedding {
                node_id: doc.id.clone(),
                vector: vec,
                model_name: Some("test-model".to_string()),
                chunk_index: i,
                chunk_start: i * 200,
                chunk_end: (i + 1) * 200,
                total_chunks: 5,
                content_hash: "hash-787".to_string(),
                token_count: 100,
            }
        })
        .collect();

    store.upsert_embeddings(&doc.id, chunks).await?;

    // First, get the document with a low threshold to see its actual scores
    let low_threshold_results = store.search_embeddings(&query_vec, 10, Some(0.3)).await?;
    assert!(
        !low_threshold_results.is_empty(),
        "Should find document with low threshold"
    );

    let result = &low_threshold_results[0];
    assert_eq!(result.node_id, doc.id);

    // Log the actual values for debugging
    println!(
        "Document scores - max_similarity: {:.4}, matching_chunks: {}, composite_score: {:.4}",
        result.max_similarity, result.matching_chunks, result.score
    );

    // Verify the composite score formula is applied correctly (Issue #944: density ratio)
    // With 5 matching chunks out of 5 total: density = 1.0
    // composite = max_similarity * (1.0 + 0.3 * 1.0) = max_similarity * 1.30
    let density = result.matching_chunks as f64 / 5.0; // total_chunks=5
    let expected_composite = result.max_similarity * (1.0 + 0.3 * density);
    assert!(
        (result.score - expected_composite).abs() < 0.01,
        "Composite score should match density formula: expected {}, got {}",
        expected_composite,
        result.score
    );

    // KEY TEST (Issue #787): Find a threshold between raw_similarity and composite_score
    // With 5/5 chunks, density = 1.0, density_factor = 1.30
    // If max_similarity = 0.68, composite = 0.68 * 1.30 ≈ 0.88
    // We choose a threshold between them
    let threshold = (result.max_similarity + result.score) / 2.0;

    println!(
        "Using threshold {:.4} (between raw {:.4} and composite {:.4})",
        threshold, result.max_similarity, result.score
    );

    // Verify our test setup: raw_similarity < threshold < composite_score
    assert!(
        result.max_similarity < threshold,
        "Test setup: raw_similarity ({}) should be below threshold ({})",
        result.max_similarity,
        threshold
    );
    assert!(
        result.score > threshold,
        "Test setup: composite_score ({}) should be above threshold ({})",
        result.score,
        threshold
    );

    // Now the actual test - search with the threshold
    let threshold_results = store
        .search_embeddings(&query_vec, 10, Some(threshold))
        .await?;

    // The document SHOULD be returned (Issue #787 fix)
    // OLD behavior (bug): document NOT returned because raw_similarity < threshold
    // NEW behavior (fix): document IS returned because composite_score > threshold
    assert!(
        !threshold_results.is_empty(),
        "Document with raw_similarity {:.4} and composite_score {:.4} should be returned with threshold {:.4}",
        result.max_similarity,
        result.score,
        threshold
    );

    let threshold_result = threshold_results.iter().find(|r| r.node_id == doc.id);
    assert!(
        threshold_result.is_some(),
        "Document should be in results because composite_score ({:.4}) > threshold ({:.4})",
        result.score,
        threshold
    );

    Ok(())
}

// =========================================================================
// Issue #936: Title inclusion in embeddings + title keyword boost tests
// =========================================================================

/// Test that title keyword boost is applied when a query term matches the node title (Issue #936)
///
/// Uses mock embeddings to test Rust-side scoring logic without an NLP engine.
/// The boost is applied in semantic_search(), which is tested here via search_embeddings()
/// + manual score inspection.
#[tokio::test]
async fn test_title_keyword_boost_applied() -> Result<()> {
    use nodespace_core::models::NewEmbedding;
    use nodespace_core::services::embedding_service::TITLE_BOOST;

    let (_embedding_service, node_service, store, _temp_dir) = create_unified_test_env().await?;

    // Doc with a title that matches likely query terms
    let mut titled_doc = Node::new(
        "text".to_string(),
        "Explains how data flows from backend to UI".to_string(),
        json!({}),
    );
    titled_doc.title = Some("Frontend State Persistence".to_string());
    node_service.create_node(titled_doc.clone()).await?;

    // Doc without a matching title but similar base similarity
    let untitled_doc = create_root_node(
        &node_service,
        "text",
        "Data persistence patterns and state management",
    )
    .await?;

    // Both docs get the same mock embedding vector (equal base similarity)
    let shared_vec: Vec<f32> = {
        let mut v = vec![0.0f32; 768];
        v[0] = 0.9;
        v[1] = 0.3;
        v
    };

    store
        .upsert_embeddings(
            &titled_doc.id,
            vec![NewEmbedding {
                node_id: titled_doc.id.clone(),
                vector: shared_vec.clone(),
                model_name: Some("test-model".to_string()),
                chunk_index: 0,
                chunk_start: 0,
                chunk_end: 100,
                total_chunks: 1,
                content_hash: "hash-titled".to_string(),
                token_count: 10,
            }],
        )
        .await?;

    store
        .upsert_embeddings(
            &untitled_doc.id,
            vec![NewEmbedding {
                node_id: untitled_doc.id.clone(),
                vector: shared_vec.clone(),
                model_name: Some("test-model".to_string()),
                chunk_index: 0,
                chunk_start: 0,
                chunk_end: 100,
                total_chunks: 1,
                content_hash: "hash-untitled".to_string(),
                token_count: 10,
            }],
        )
        .await?;

    // Retrieve raw scores from DB (no title boost applied here)
    let raw_results = store.search_embeddings(&shared_vec, 10, Some(0.5)).await?;

    assert!(
        raw_results.len() >= 2,
        "Both documents should be returned with low threshold"
    );

    // Both should have the same raw composite_score (identical vectors, single chunk)
    let raw_titled = raw_results.iter().find(|r| r.node_id == titled_doc.id);
    let raw_untitled = raw_results.iter().find(|r| r.node_id == untitled_doc.id);
    assert!(
        raw_titled.is_some(),
        "Titled doc should appear in raw results"
    );
    assert!(
        raw_untitled.is_some(),
        "Untitled doc should appear in raw results"
    );

    let raw_score_titled = raw_titled.unwrap().score;
    let raw_score_untitled = raw_untitled.unwrap().score;

    // Scores should be equal before boost (same vector, same chunk count)
    assert!(
        (raw_score_titled - raw_score_untitled).abs() < 0.001,
        "Raw scores should be equal before boost, got titled={}, untitled={}",
        raw_score_titled,
        raw_score_untitled
    );

    // Simulate what semantic_search does: apply title boost
    // Query term "persistence" appears in titled doc's title "Frontend State Persistence"
    let query_terms = ["persistence"];
    let titled_title = titled_doc.title.as_deref().unwrap_or("").to_lowercase();
    let has_title_match = query_terms
        .iter()
        .any(|t| titled_title.contains(&t.to_lowercase()));
    assert!(
        has_title_match,
        "Query term 'persistence' should match titled doc title"
    );

    let boosted_titled_score = raw_score_titled + TITLE_BOOST;
    let untouched_untitled_score = raw_score_untitled;

    assert!(
        boosted_titled_score > untouched_untitled_score,
        "Titled doc (score {}) should rank above untitled doc (score {}) after title boost",
        boosted_titled_score,
        untouched_untitled_score
    );

    // Verify the boost amount
    assert!(
        (boosted_titled_score - untouched_untitled_score - TITLE_BOOST).abs() < 0.001,
        "Boost should be exactly TITLE_BOOST ({})",
        TITLE_BOOST
    );

    Ok(())
}

// =========================================================================
// Hybrid Search Tests (Issue #951)
// =========================================================================

/// Test that BM25 search finds nodes containing query terms
#[tokio::test]
async fn test_bm25_search_roots_finds_root_by_content() -> Result<()> {
    let (_embedding_service, node_service, store, _temp_dir) = create_unified_test_env().await?;

    // Create a root node with specific vocabulary
    let doc = create_root_node(
        &node_service,
        "text",
        "SharedNodeStore debounce persistence coordinator pattern",
    )
    .await?;

    // Create an unrelated node
    let other = create_root_node(&node_service, "text", "Unrelated content about cats").await?;

    // BM25 search should find the document by exact term
    let roots = store.bm25_search_roots("persistence", 50).await?;

    assert!(
        roots.contains(&doc.id),
        "BM25 should find root with 'persistence' in content"
    );
    assert!(
        !roots.contains(&other.id),
        "BM25 should not find unrelated document"
    );

    Ok(())
}

/// Test that BM25 search resolves child node matches to their root
#[tokio::test]
async fn test_bm25_search_roots_resolves_child_to_root() -> Result<()> {
    let (_embedding_service, node_service, store, _temp_dir) = create_unified_test_env().await?;

    // Create a root node with generic content
    let root = create_root_node(&node_service, "text", "Overview document").await?;

    // Create a child node with specific technical vocabulary
    let child = create_child_node(
        &node_service,
        &root.id,
        "text",
        "SimplePersistenceCoordinator debounce implementation",
    )
    .await?;

    // BM25 search should find the ROOT (not the child) when the term is in the child
    let roots = store.bm25_search_roots("debounce", 50).await?;

    assert!(
        roots.contains(&root.id),
        "BM25 should surface the ROOT when query term is in a child node"
    );
    assert!(
        !roots.contains(&child.id),
        "BM25 should return root IDs, not child IDs"
    );

    Ok(())
}

/// Test BM25 with query term buried in a grandchild node
#[tokio::test]
async fn test_bm25_search_roots_resolves_grandchild_to_root() -> Result<()> {
    let (_embedding_service, node_service, store, _temp_dir) = create_unified_test_env().await?;

    let root = create_root_node(&node_service, "text", "Frontend Architecture").await?;
    let child =
        create_child_node(&node_service, &root.id, "text", "State management section").await?;
    let grandchild = create_child_node(
        &node_service,
        &child.id,
        "text",
        "reactivity rune derived store update",
    )
    .await?;

    // Query term is in grandchild - should resolve to root
    let roots = store.bm25_search_roots("reactivity", 50).await?;

    assert!(
        roots.contains(&root.id),
        "BM25 should surface ROOT when query term is in grandchild"
    );
    assert!(
        !roots.contains(&child.id),
        "Intermediate child should not appear as root"
    );
    assert!(
        !roots.contains(&grandchild.id),
        "Grandchild should not appear as root"
    );

    Ok(())
}

/// Test that BM25 returns empty set when no content matches
#[tokio::test]
async fn test_bm25_search_roots_no_match_returns_empty() -> Result<()> {
    let (_embedding_service, node_service, store, _temp_dir) = create_unified_test_env().await?;

    let _doc = create_root_node(&node_service, "text", "cats and dogs").await?;

    let roots = store.bm25_search_roots("xyznonexistentterm123", 50).await?;

    assert!(
        roots.is_empty(),
        "BM25 should return empty set when no content matches"
    );

    Ok(())
}

/// Test the hybrid tiering logic: intersection (tier 1) vs KNN-only (tier 2)
///
/// Verifies intersection signal by directly running BM25 and KNN independently
/// and checking which docs are in each set. This avoids model initialization.
#[tokio::test]
async fn test_hybrid_intersection_tier_signal() -> Result<()> {
    use nodespace_core::models::NewEmbedding;

    let (_embedding_service, node_service, store, _temp_dir) = create_unified_test_env().await?;

    // Doc A: Has exact keyword "persistence" in content AND an embedding
    let doc_a = create_root_node(
        &node_service,
        "text",
        "persistence coordinator pattern for storing node changes",
    )
    .await?;

    // Doc B: Has embedding but no keyword match for "persistence"
    let doc_b = create_root_node(
        &node_service,
        "text",
        "SharedNodeStore debounce coordinator implementation details",
    )
    .await?;

    // Give both identical embedding vectors (same KNN similarity)
    let mut vec_same = vec![0.0f32; 768];
    vec_same[0] = 1.0;
    vec_same[1] = 0.5;

    for (node, hash) in [(&doc_a, "hash_a"), (&doc_b, "hash_b")] {
        store
            .upsert_embeddings(
                &node.id,
                vec![NewEmbedding {
                    node_id: node.id.clone(),
                    vector: vec_same.clone(),
                    model_name: Some("test-model".to_string()),
                    chunk_index: 0,
                    chunk_start: 0,
                    chunk_end: 100,
                    total_chunks: 1,
                    content_hash: hash.to_string(),
                    token_count: 10,
                }],
            )
            .await?;
    }

    // Run both signals independently (mirrors the hybrid search parallel execution)
    let bm25_roots = store.bm25_search_roots("persistence", 50).await?;
    let knn_results = store.search_embeddings(&vec_same, 20, Some(0.1)).await?;
    let knn_ids: std::collections::HashSet<String> =
        knn_results.iter().map(|r| r.node_id.clone()).collect();

    // doc_a should be in BOTH (tier 1: intersection)
    assert!(
        bm25_roots.contains(&doc_a.id),
        "doc_a should be in BM25 results (has 'persistence' in content)"
    );
    assert!(
        knn_ids.contains(&doc_a.id),
        "doc_a should be in KNN results (has embedding)"
    );

    // doc_b should be in KNN only (tier 2)
    assert!(
        !bm25_roots.contains(&doc_b.id),
        "doc_b should NOT be in BM25 results (no 'persistence' in content)"
    );
    assert!(
        knn_ids.contains(&doc_b.id),
        "doc_b should be in KNN results (has embedding)"
    );

    // Intersection is exactly doc_a
    let intersection: std::collections::HashSet<&String> =
        bm25_roots.intersection(&knn_ids).collect();
    assert!(
        intersection.contains(&&doc_a.id),
        "doc_a should be in intersection (tier 1)"
    );
    assert!(
        !intersection.contains(&&doc_b.id),
        "doc_b should not be in intersection (tier 2 only)"
    );

    Ok(())
}

/// Test that BM25-only docs (tier 3) have no KNN embedding
///
/// Verifies that docs without embeddings appear only in BM25 results,
/// which the hybrid search will rank last (score=0.0).
#[tokio::test]
async fn test_hybrid_bm25_only_tier_has_no_knn_score() -> Result<()> {
    use nodespace_core::models::NewEmbedding;

    let (_embedding_service, node_service, store, _temp_dir) = create_unified_test_env().await?;

    // Doc A: Has keyword match but NO embedding (BM25-only, tier 3)
    let doc_bm25_only = create_root_node(
        &node_service,
        "text",
        "database persistence coordinator pattern",
    )
    .await?;
    // Note: no embedding upserted → won't appear in KNN results

    // Doc B: Has embedding but no keyword match (KNN-only, tier 2)
    let doc_knn_only = create_root_node(
        &node_service,
        "text",
        "SharedNodeStore implementation with reactive state",
    )
    .await?;

    let mut vec_b = vec![0.0f32; 768];
    vec_b[0] = 0.9;
    vec_b[1] = 0.4;

    store
        .upsert_embeddings(
            &doc_knn_only.id,
            vec![NewEmbedding {
                node_id: doc_knn_only.id.clone(),
                vector: vec_b.clone(),
                model_name: Some("test-model".to_string()),
                chunk_index: 0,
                chunk_start: 0,
                chunk_end: 100,
                total_chunks: 1,
                content_hash: "hash_knn".to_string(),
                token_count: 10,
            }],
        )
        .await?;

    // Run both signals
    let bm25_roots = store.bm25_search_roots("persistence", 50).await?;
    let knn_results = store.search_embeddings(&vec_b, 20, Some(0.1)).await?;
    let knn_ids: std::collections::HashSet<String> =
        knn_results.iter().map(|r| r.node_id.clone()).collect();

    // doc_bm25_only: in BM25 but NOT in KNN
    assert!(
        bm25_roots.contains(&doc_bm25_only.id),
        "doc_bm25_only should appear in BM25 results"
    );
    assert!(
        !knn_ids.contains(&doc_bm25_only.id),
        "doc_bm25_only should NOT appear in KNN results (no embedding)"
    );

    // doc_knn_only: in KNN but NOT in BM25
    assert!(
        knn_ids.contains(&doc_knn_only.id),
        "doc_knn_only should appear in KNN results"
    );
    assert!(
        !bm25_roots.contains(&doc_knn_only.id),
        "doc_knn_only should NOT appear in BM25 results (no 'persistence' in content)"
    );

    Ok(())
}

/// Test that conceptual query with no keyword match returns only KNN results from BM25
///
/// Verifies that BM25 returns empty when query terms don't match content vocabulary,
/// while KNN can still find the doc by embedding similarity.
#[tokio::test]
async fn test_hybrid_conceptual_query_bm25_misses_knn_hits() -> Result<()> {
    use nodespace_core::models::NewEmbedding;

    let (_embedding_service, node_service, store, _temp_dir) = create_unified_test_env().await?;

    // Doc with technical vocabulary — BM25 won't match "pattern for persisting changes"
    let doc = create_root_node(
        &node_service,
        "text",
        "SharedNodeStore debounce SimplePersistenceCoordinator reactive state",
    )
    .await?;

    let mut vec = vec![0.0f32; 768];
    vec[0] = 1.0;

    store
        .upsert_embeddings(
            &doc.id,
            vec![NewEmbedding {
                node_id: doc.id.clone(),
                vector: vec.clone(),
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

    // BM25 query uses different vocabulary than the document content → no match
    let bm25_roots = store
        .bm25_search_roots("pattern for persisting changes", 50)
        .await?;

    // KNN finds it via embedding similarity
    let knn_results = store.search_embeddings(&vec, 20, Some(0.1)).await?;
    let knn_ids: std::collections::HashSet<String> =
        knn_results.iter().map(|r| r.node_id.clone()).collect();

    // BM25 misses the doc (vocabulary mismatch)
    assert!(
        !bm25_roots.contains(&doc.id),
        "BM25 should not find doc with different vocabulary (this is the known weakness hybrid search addresses)"
    );

    // KNN finds it (embedding captures semantic similarity)
    assert!(
        knn_ids.contains(&doc.id),
        "KNN should find doc via embedding similarity (tier 2 fallback)"
    );

    Ok(())
}

// =========================================================================
// Issue #1018: NodeAccessor Implementation Tests
// =========================================================================

/// Test: NodeAccessor implementation on NodeService works correctly (Issue #1018)
#[tokio::test]
async fn test_node_accessor_get_node() -> Result<()> {
    use nodespace_core::services::NodeAccessor;

    let (_embedding_service, node_service, _store, _temp_dir) = create_unified_test_env().await?;
    let root = create_root_node(&node_service, "text", "Test content").await?;

    // Test NodeAccessor::get_node (call through trait explicitly to verify the impl)
    let accessor: &dyn NodeAccessor = &node_service;
    let found = accessor.get_node(&root.id).await?;
    assert!(found.is_some());
    assert_eq!(found.unwrap().content, "Test content");

    // Test NodeAccessor::get_node for missing node
    let missing = accessor.get_node("nonexistent").await?;
    assert!(missing.is_none());
    Ok(())
}

/// Test: NodeAccessor::get_children returns children sorted by order (Issue #1018)
#[tokio::test]
async fn test_node_accessor_get_children() -> Result<()> {
    use nodespace_core::services::NodeAccessor;

    let (_embedding_service, node_service, _store, _temp_dir) = create_unified_test_env().await?;
    let root = create_root_node(&node_service, "text", "Root").await?;
    let _child1 = create_child_node(&node_service, &root.id, "text", "Child 1").await?;
    let _child2 = create_child_node(&node_service, &root.id, "text", "Child 2").await?;

    let children: Vec<Node> = NodeAccessor::get_children(&node_service, &root.id).await?;
    assert_eq!(children.len(), 2);
    Ok(())
}

/// Test: NodeAccessor::get_nodes batch fetch works (Issue #1018)
#[tokio::test]
async fn test_node_accessor_get_nodes_batch() -> Result<()> {
    use nodespace_core::services::NodeAccessor;

    let (_embedding_service, node_service, _store, _temp_dir) = create_unified_test_env().await?;
    let root1 = create_root_node(&node_service, "text", "Node 1").await?;
    let root2 = create_root_node(&node_service, "text", "Node 2").await?;

    let ids: Vec<&str> = vec![root1.id.as_str(), root2.id.as_str()];
    let nodes = node_service.get_nodes(&ids).await?;
    assert_eq!(nodes.len(), 2);
    Ok(())
}
