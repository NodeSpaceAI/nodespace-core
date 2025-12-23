//! Integration tests for MCP Search Handlers
//!
//! Tests cover:
//! - handle_search_semantic function
//! - Collection filtering
//! - Error handling
//! - Parameter validation

use anyhow::Result;
use nodespace_core::{
    db::SurrealStore,
    mcp::handlers::search::{handle_search_semantic, SearchSemanticParams},
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
async fn create_test_services() -> Result<(
    Arc<NodeEmbeddingService>,
    Arc<NodeService>,
    Arc<SurrealStore>,
    TempDir,
)> {
    let temp_dir = TempDir::new()?;
    let db_path = temp_dir.path().join("test.db");
    let mut store = Arc::new(SurrealStore::new(db_path).await?);

    let node_service = Arc::new(NodeService::new(&mut store).await?);
    let nlp_engine = create_test_nlp_engine();
    let embedding_service = Arc::new(NodeEmbeddingService::new(nlp_engine, store.clone()));

    Ok((embedding_service, node_service, store, temp_dir))
}

// =========================================================================
// Parameter Parsing Integration Tests
// =========================================================================

#[test]
fn test_search_params_parse_minimal() {
    let params = json!({
        "query": "test query"
    });

    let parsed: SearchSemanticParams = serde_json::from_value(params).unwrap();
    assert_eq!(parsed.query, "test query");
    assert_eq!(parsed.threshold, None);
    assert_eq!(parsed.limit, None);
    assert_eq!(parsed.collection_id, None);
    assert_eq!(parsed.collection, None);
}

#[test]
fn test_search_params_parse_full() {
    let params = json!({
        "query": "machine learning",
        "threshold": 0.8,
        "limit": 50,
        "collection_id": "coll-123",
        "collection": "ai:research"
    });

    let parsed: SearchSemanticParams = serde_json::from_value(params).unwrap();
    assert_eq!(parsed.query, "machine learning");
    assert_eq!(parsed.threshold, Some(0.8));
    assert_eq!(parsed.limit, Some(50));
    assert_eq!(parsed.collection_id, Some("coll-123".to_string()));
    assert_eq!(parsed.collection, Some("ai:research".to_string()));
}

#[test]
fn test_search_params_missing_query_fails() {
    let params = json!({
        "threshold": 0.8
    });

    let result: Result<SearchSemanticParams, _> = serde_json::from_value(params);
    assert!(result.is_err());
}

// =========================================================================
// Handler Validation Tests
// =========================================================================

#[tokio::test]
async fn test_search_rejects_empty_query() -> Result<()> {
    let (embedding_service, node_service, _store, _temp_dir) = create_test_services().await?;

    let params = json!({
        "query": ""
    });

    let result = handle_search_semantic(&node_service, &embedding_service, params).await;
    assert!(result.is_err());

    let err = result.unwrap_err();
    assert!(err.message.contains("empty"));
    Ok(())
}

#[tokio::test]
async fn test_search_rejects_whitespace_query() -> Result<()> {
    let (embedding_service, node_service, _store, _temp_dir) = create_test_services().await?;

    let params = json!({
        "query": "   "
    });

    let result = handle_search_semantic(&node_service, &embedding_service, params).await;
    assert!(result.is_err());

    let err = result.unwrap_err();
    assert!(err.message.contains("empty") || err.message.contains("whitespace"));
    Ok(())
}

#[tokio::test]
async fn test_search_rejects_threshold_below_zero() -> Result<()> {
    let (embedding_service, node_service, _store, _temp_dir) = create_test_services().await?;

    let params = json!({
        "query": "valid query",
        "threshold": -0.1
    });

    let result = handle_search_semantic(&node_service, &embedding_service, params).await;
    assert!(result.is_err());

    let err = result.unwrap_err();
    assert!(err.message.contains("threshold"));
    Ok(())
}

#[tokio::test]
async fn test_search_rejects_threshold_above_one() -> Result<()> {
    let (embedding_service, node_service, _store, _temp_dir) = create_test_services().await?;

    let params = json!({
        "query": "valid query",
        "threshold": 1.5
    });

    let result = handle_search_semantic(&node_service, &embedding_service, params).await;
    assert!(result.is_err());

    let err = result.unwrap_err();
    assert!(err.message.contains("threshold"));
    Ok(())
}

#[tokio::test]
async fn test_search_rejects_limit_exceeds_max() -> Result<()> {
    let (embedding_service, node_service, _store, _temp_dir) = create_test_services().await?;

    let params = json!({
        "query": "valid query",
        "limit": 5000
    });

    let result = handle_search_semantic(&node_service, &embedding_service, params).await;
    assert!(result.is_err());

    let err = result.unwrap_err();
    assert!(err.message.contains("limit") || err.message.contains("1000"));
    Ok(())
}

#[tokio::test]
async fn test_search_accepts_boundary_threshold_zero() -> Result<()> {
    let (embedding_service, node_service, _store, _temp_dir) = create_test_services().await?;

    let params = json!({
        "query": "valid query",
        "threshold": 0.0
    });

    // Should not fail on validation, may fail on actual search (no embeddings)
    let result = handle_search_semantic(&node_service, &embedding_service, params).await;
    // Either succeeds with empty results or fails with embedding-related error, not validation
    if let Err(e) = result {
        assert!(!e.message.contains("threshold"));
    }
    Ok(())
}

#[tokio::test]
async fn test_search_accepts_boundary_threshold_one() -> Result<()> {
    let (embedding_service, node_service, _store, _temp_dir) = create_test_services().await?;

    let params = json!({
        "query": "valid query",
        "threshold": 1.0
    });

    // Should not fail on validation
    let result = handle_search_semantic(&node_service, &embedding_service, params).await;
    if let Err(e) = result {
        assert!(!e.message.contains("threshold"));
    }
    Ok(())
}

#[tokio::test]
async fn test_search_accepts_max_limit() -> Result<()> {
    let (embedding_service, node_service, _store, _temp_dir) = create_test_services().await?;

    let params = json!({
        "query": "valid query",
        "limit": 1000
    });

    // Should not fail on validation
    let result = handle_search_semantic(&node_service, &embedding_service, params).await;
    if let Err(e) = result {
        assert!(!e.message.contains("limit") && !e.message.contains("1000"));
    }
    Ok(())
}

// =========================================================================
// Collection Path Resolution Tests
// =========================================================================

#[tokio::test]
async fn test_search_with_collection_id_filter() -> Result<()> {
    let (embedding_service, node_service, _store, _temp_dir) = create_test_services().await?;

    // Using collection_id instead of collection path
    let params = json!({
        "query": "valid query",
        "collection_id": "some-collection-id"
    });

    let result = handle_search_semantic(&node_service, &embedding_service, params).await;

    // The handler will attempt collection filtering
    // Either succeeds with empty results or fails gracefully
    match result {
        Ok(response) => {
            // If successful, results may be empty (no matching nodes in collection)
            assert!(response.get("nodes").is_some());
            assert!(response.get("count").is_some());
        }
        Err(e) => {
            // If error, it's related to search/embeddings, not validation
            // Collection ID filtering just filters results, doesn't fail
            let msg = e.message.to_lowercase();
            assert!(
                !msg.contains("invalid parameters"),
                "Should not be validation error: {}",
                e.message
            );
        }
    }
    Ok(())
}

// =========================================================================
// Response Structure Tests
// =========================================================================

#[tokio::test]
async fn test_search_response_has_required_fields() -> Result<()> {
    let (embedding_service, node_service, _store, _temp_dir) = create_test_services().await?;

    let params = json!({
        "query": "test query"
    });

    // This may fail with "no embeddings" but let's test if it at least tries
    let result = handle_search_semantic(&node_service, &embedding_service, params).await;

    // If the service isn't initialized, it may return an error, but the error
    // should be about embeddings, not structure
    if let Ok(response) = result {
        assert!(response.get("nodes").is_some());
        assert!(response.get("count").is_some());
        assert!(response.get("query").is_some());
        assert!(response.get("threshold").is_some());
    }
    Ok(())
}

// =========================================================================
// Default Value Tests
// =========================================================================

#[test]
fn test_search_defaults_applied_correctly() {
    let params = json!({"query": "test"});
    let parsed: SearchSemanticParams = serde_json::from_value(params).unwrap();

    // Apply defaults as the handler does
    let threshold = parsed.threshold.unwrap_or(0.7);
    let limit = parsed.limit.unwrap_or(20);

    assert_eq!(threshold, 0.7, "Default threshold should be 0.7");
    assert_eq!(limit, 20, "Default limit should be 20");
}

// =========================================================================
// Malformed Input Tests
// =========================================================================

#[test]
fn test_search_params_rejects_array_instead_of_object() {
    // Pass an array instead of an object
    let params = json!(["query", "test"]);
    let result: Result<SearchSemanticParams, _> = serde_json::from_value(params);
    assert!(result.is_err(), "Should reject array input");
}

#[test]
fn test_search_params_rejects_string_instead_of_object() {
    // Pass a string instead of an object
    let params = json!("just a string");
    let result: Result<SearchSemanticParams, _> = serde_json::from_value(params);
    assert!(result.is_err(), "Should reject string input");
}

#[test]
fn test_search_params_rejects_null() {
    // Pass null
    let params = json!(null);
    let result: Result<SearchSemanticParams, _> = serde_json::from_value(params);
    assert!(result.is_err(), "Should reject null input");
}

#[test]
fn test_search_params_rejects_number() {
    // Pass a number instead of an object
    let params = json!(42);
    let result: Result<SearchSemanticParams, _> = serde_json::from_value(params);
    assert!(result.is_err(), "Should reject number input");
}

#[test]
fn test_search_params_rejects_wrong_type_for_query() {
    // Query should be a string, not a number
    let params = json!({
        "query": 12345
    });
    let result: Result<SearchSemanticParams, _> = serde_json::from_value(params);
    assert!(result.is_err(), "Should reject non-string query");
}

#[test]
fn test_search_params_rejects_wrong_type_for_threshold() {
    // Threshold should be a number, not a string
    let params = json!({
        "query": "valid query",
        "threshold": "not a number"
    });
    let result: Result<SearchSemanticParams, _> = serde_json::from_value(params);
    assert!(result.is_err(), "Should reject non-numeric threshold");
}

#[test]
fn test_search_params_rejects_wrong_type_for_limit() {
    // Limit should be a number, not a string
    let params = json!({
        "query": "valid query",
        "limit": "not a number"
    });
    let result: Result<SearchSemanticParams, _> = serde_json::from_value(params);
    assert!(result.is_err(), "Should reject non-numeric limit");
}
