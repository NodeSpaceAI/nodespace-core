//! Integration tests for InboundRelationshipCache
//!
//! Tests cover:
//! - Cache creation and initialization
//! - Cache refresh mechanics
//! - TTL-based invalidation
//! - Event-driven invalidation
//! - Cache statistics
//! - Concurrent access patterns

use anyhow::Result;
use nodespace_core::{
    db::SurrealStore,
    services::{relationship_cache::InboundRelationshipCache, NodeService},
};
use std::sync::Arc;
use std::time::Duration;
use tempfile::TempDir;

/// Test helper: Create a test environment
async fn create_test_env() -> Result<(Arc<SurrealStore>, NodeService, TempDir)> {
    let temp_dir = TempDir::new()?;
    let db_path = temp_dir.path().join("test.db");
    let mut store = Arc::new(SurrealStore::new(db_path).await?);
    let node_service = NodeService::new(&mut store).await?;

    Ok((store, node_service, temp_dir))
}

// =========================================================================
// Cache Creation Tests
// =========================================================================

#[tokio::test]
async fn test_cache_creation() -> Result<()> {
    let (store, _node_service, _temp_dir) = create_test_env().await?;

    let cache = InboundRelationshipCache::new(store);

    // Cache should start empty
    let stats = cache.stats().await;
    assert!(stats.is_stale, "New cache should be stale");
    assert!(
        stats.last_refresh.is_none(),
        "New cache should not have last_refresh"
    );
    Ok(())
}

#[tokio::test]
async fn test_cache_creation_with_custom_ttl() -> Result<()> {
    let (store, _node_service, _temp_dir) = create_test_env().await?;

    let ttl = Duration::from_secs(10);
    let cache = InboundRelationshipCache::with_ttl(store, ttl);

    let stats = cache.stats().await;
    assert!(stats.is_stale, "New cache should be stale");
    Ok(())
}

// =========================================================================
// Cache Refresh Tests
// =========================================================================

#[tokio::test]
async fn test_cache_refreshes_on_first_access() -> Result<()> {
    let (store, _node_service, _temp_dir) = create_test_env().await?;

    let cache = InboundRelationshipCache::new(store);

    // First access should trigger refresh
    let relationships = cache.get_inbound_relationships("customer").await?;

    // Should return empty vec (no schemas yet)
    assert!(relationships.is_empty());

    // Stats should show refresh happened
    let stats = cache.stats().await;
    assert!(
        stats.last_refresh.is_some(),
        "Cache should have refreshed on first access"
    );
    Ok(())
}

#[tokio::test]
async fn test_force_refresh() -> Result<()> {
    let (store, _node_service, _temp_dir) = create_test_env().await?;

    let cache = InboundRelationshipCache::new(store);

    // Force refresh
    cache.force_refresh().await?;

    let stats = cache.stats().await;
    assert!(
        stats.last_refresh.is_some(),
        "Force refresh should set last_refresh"
    );
    assert!(
        !stats.is_stale,
        "Cache should not be stale after force refresh"
    );
    Ok(())
}

// =========================================================================
// Invalidation Tests
// =========================================================================

#[tokio::test]
async fn test_invalidate_marks_cache_stale() -> Result<()> {
    let (store, _node_service, _temp_dir) = create_test_env().await?;

    let cache = InboundRelationshipCache::new(store);

    // First, force refresh
    cache.force_refresh().await?;

    let stats_before = cache.stats().await;
    assert!(
        !stats_before.is_stale,
        "Cache should not be stale after refresh"
    );

    // Invalidate
    cache.invalidate();

    let stats_after = cache.stats().await;
    assert!(
        stats_after.is_stale,
        "Cache should be stale after invalidation"
    );
    Ok(())
}

#[tokio::test]
async fn test_ttl_based_invalidation() -> Result<()> {
    let (store, _node_service, _temp_dir) = create_test_env().await?;

    // Use very short TTL
    let ttl = Duration::from_millis(10);
    let cache = InboundRelationshipCache::with_ttl(store, ttl);

    // Force refresh
    cache.force_refresh().await?;

    let stats_before = cache.stats().await;
    assert!(
        !stats_before.is_stale,
        "Cache should not be stale immediately after refresh"
    );

    // Wait for TTL to expire
    tokio::time::sleep(Duration::from_millis(20)).await;

    let stats_after = cache.stats().await;
    assert!(
        stats_after.is_stale,
        "Cache should be stale after TTL expires"
    );
    Ok(())
}

// =========================================================================
// Get All Relationships Tests
// =========================================================================

#[tokio::test]
async fn test_get_all_inbound_relationships_empty() -> Result<()> {
    let (store, _node_service, _temp_dir) = create_test_env().await?;

    let cache = InboundRelationshipCache::new(store);

    let all = cache.get_all_inbound_relationships().await?;

    // Should be empty initially
    assert!(all.is_empty());
    Ok(())
}

// =========================================================================
// Statistics Tests
// =========================================================================

#[tokio::test]
async fn test_cache_stats_empty() -> Result<()> {
    let (store, _node_service, _temp_dir) = create_test_env().await?;

    let cache = InboundRelationshipCache::new(store);

    // Access to trigger refresh
    let _ = cache.get_inbound_relationships("any").await?;

    let stats = cache.stats().await;

    assert_eq!(
        stats.target_types, 0,
        "Empty cache should have 0 target types"
    );
    assert_eq!(
        stats.total_relationships, 0,
        "Empty cache should have 0 relationships"
    );
    assert!(
        stats.last_refresh.is_some(),
        "Stats should show last refresh"
    );
    Ok(())
}

// =========================================================================
// Concurrent Access Tests
// =========================================================================

#[tokio::test]
async fn test_concurrent_reads() -> Result<()> {
    let (store, _node_service, _temp_dir) = create_test_env().await?;

    let cache = Arc::new(InboundRelationshipCache::new(store));

    // Spawn multiple concurrent reads
    let mut handles = Vec::new();
    for i in 0..10 {
        let cache_clone = cache.clone();
        let target = format!("type_{}", i % 3);
        handles.push(tokio::spawn(async move {
            cache_clone.get_inbound_relationships(&target).await
        }));
    }

    // All should succeed
    for handle in handles {
        let result = handle.await?;
        assert!(result.is_ok());
    }
    Ok(())
}

#[tokio::test]
async fn test_concurrent_read_and_invalidate() -> Result<()> {
    let (store, _node_service, _temp_dir) = create_test_env().await?;

    let cache = Arc::new(InboundRelationshipCache::new(store));

    // First trigger refresh
    cache.force_refresh().await?;

    // Spawn reads concurrently
    let mut read_handles = Vec::new();
    for _ in 0..5 {
        let cache_clone = cache.clone();
        read_handles.push(tokio::spawn(async move {
            cache_clone.get_inbound_relationships("customer").await
        }));
    }

    // Spawn invalidations concurrently (separate vector due to different return type)
    let mut invalidate_handles = Vec::new();
    for _ in 0..3 {
        let cache_clone = cache.clone();
        invalidate_handles.push(tokio::spawn(async move {
            cache_clone.invalidate();
        }));
    }

    // All read operations should complete without panic
    for handle in read_handles {
        let _ = handle.await?;
    }

    // All invalidate operations should complete without panic
    for handle in invalidate_handles {
        handle.await?;
    }
    Ok(())
}

// =========================================================================
// InboundRelationship Struct Tests
// =========================================================================

#[test]
fn test_inbound_relationship_equality() {
    use nodespace_core::models::schema::RelationshipCardinality;
    use nodespace_core::services::relationship_cache::InboundRelationship;

    let rel1 = InboundRelationship {
        source_type: "invoice".to_string(),
        relationship_name: "billed_to".to_string(),
        reverse_name: Some("invoices".to_string()),
        cardinality: RelationshipCardinality::One,
        reverse_cardinality: Some(RelationshipCardinality::Many),
        edge_table: "invoice_billed_to_customer".to_string(),
        description: None,
    };

    let rel2 = InboundRelationship {
        source_type: "invoice".to_string(),
        relationship_name: "billed_to".to_string(),
        reverse_name: Some("invoices".to_string()),
        cardinality: RelationshipCardinality::One,
        reverse_cardinality: Some(RelationshipCardinality::Many),
        edge_table: "invoice_billed_to_customer".to_string(),
        description: None,
    };

    assert_eq!(rel1, rel2);
}

#[test]
fn test_inbound_relationship_clone() {
    use nodespace_core::models::schema::RelationshipCardinality;
    use nodespace_core::services::relationship_cache::InboundRelationship;

    let rel = InboundRelationship {
        source_type: "task".to_string(),
        relationship_name: "assigned_to".to_string(),
        reverse_name: Some("tasks".to_string()),
        cardinality: RelationshipCardinality::Many,
        reverse_cardinality: Some(RelationshipCardinality::Many),
        edge_table: "task_assigned_to_person".to_string(),
        description: Some("Assignment relationship".to_string()),
    };

    let cloned = rel.clone();
    assert_eq!(rel, cloned);
}

#[test]
fn test_inbound_relationship_json_round_trip() {
    use nodespace_core::models::schema::RelationshipCardinality;
    use nodespace_core::services::relationship_cache::InboundRelationship;

    let rel = InboundRelationship {
        source_type: "order".to_string(),
        relationship_name: "items".to_string(),
        reverse_name: Some("orders".to_string()),
        cardinality: RelationshipCardinality::Many,
        reverse_cardinality: Some(RelationshipCardinality::One),
        edge_table: "order_items_product".to_string(),
        description: Some("Order contains products".to_string()),
    };

    let json = serde_json::to_string(&rel).unwrap();
    let parsed: InboundRelationship = serde_json::from_str(&json).unwrap();

    assert_eq!(rel, parsed);
}
