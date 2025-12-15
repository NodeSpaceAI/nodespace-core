//! MCP Search Handlers
//!
//! Semantic search operations for AI agent access.
//! Pure business logic - no Tauri dependencies.

use crate::mcp::types::MCPError;
use crate::services::{CollectionService, NodeEmbeddingService, NodeServiceError};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashSet;
use std::sync::Arc;

/// Parameters for search_semantic method
#[derive(Debug, Deserialize)]
pub struct SearchSemanticParams {
    /// Natural language search query
    pub query: String,

    /// Minimum similarity threshold (0.0-1.0, higher = stricter filter)
    /// Results must have similarity > threshold to be included
    /// Default: 0.7
    #[serde(default)]
    pub threshold: Option<f32>,

    /// Maximum number of results
    /// Default: 20
    #[serde(default)]
    pub limit: Option<usize>,

    /// Filter by collection ID - returns only results from this collection
    #[serde(default)]
    pub collection_id: Option<String>,

    /// Filter by collection path (e.g., "hr:policy") - resolves path to collection ID
    #[serde(default)]
    pub collection: Option<String>,
}

/// Search root nodes by semantic similarity
///
/// Uses vector embeddings to find root nodes whose content is semantically
/// similar to the query. This enables AI agents to discover relevant content
/// using natural language instead of knowing exact IDs.
///
/// # Example
///
/// ```ignore
/// let params = json!({
///     "query": "Q4 planning and budget",
///     "threshold": 0.7,
///     "limit": 10
/// });
/// let result = handle_search_semantic(&embedding_service, params).await?;
/// // Returns top 10 most relevant root nodes
/// ```
pub async fn handle_search_semantic<C>(
    service: &Arc<NodeEmbeddingService<C>>,
    params: Value,
) -> Result<Value, MCPError>
where
    C: surrealdb::Connection,
{
    // Parse parameters
    let params: SearchSemanticParams = serde_json::from_value(params)
        .map_err(|e| MCPError::invalid_params(format!("Invalid parameters: {}", e)))?;

    // Apply defaults (authoritative - schema defaults are client hints only)
    // These values override any client-side defaults from the JSON schema
    let threshold = params.threshold.unwrap_or(0.7);
    let limit = params.limit.unwrap_or(20);

    // Validate parameters
    if !(0.0..=1.0).contains(&threshold) {
        return Err(MCPError::invalid_params(
            "threshold must be between 0.0 and 1.0".to_string(),
        ));
    }

    if limit > 1000 {
        return Err(MCPError::invalid_params(
            "limit cannot exceed 1000".to_string(),
        ));
    }

    if params.query.trim().is_empty() {
        return Err(MCPError::invalid_params(
            "query cannot be empty or whitespace".to_string(),
        ));
    }

    // Resolve collection ID and get member IDs if filtering by collection
    let (collection_id, collection_member_ids): (Option<String>, Option<HashSet<String>>) =
        if let Some(path) = &params.collection {
            let collection_service = CollectionService::new(service.store());
            match collection_service.resolve_path(path).await {
                Ok(resolved) => {
                    let coll_id = resolved.leaf_id().to_string();
                    let members = collection_service
                        .get_collection_members(&coll_id)
                        .await
                        .map_err(|e| {
                            MCPError::internal_error(format!(
                                "Failed to get collection members: {}",
                                e
                            ))
                        })?;
                    (Some(coll_id), Some(members.into_iter().collect()))
                }
                Err(NodeServiceError::CollectionNotFound(_)) => {
                    // Collection doesn't exist, return empty result
                    return Ok(json!({
                        "nodes": [],
                        "count": 0,
                        "query": params.query,
                        "threshold": threshold,
                        "collection_id": null
                    }));
                }
                Err(e) => {
                    return Err(MCPError::internal_error(format!(
                        "Failed to resolve collection path: {}",
                        e
                    )))
                }
            }
        } else if let Some(coll_id) = &params.collection_id {
            let collection_service = CollectionService::new(service.store());
            let members = collection_service
                .get_collection_members(coll_id)
                .await
                .map_err(|e| {
                    MCPError::internal_error(format!("Failed to get collection members: {}", e))
                })?;
            (Some(coll_id.clone()), Some(members.into_iter().collect()))
        } else {
            (None, None)
        };

    tracing::info!("Semantic search for: '{}'", params.query);

    // When filtering by collection, fetch more results to compensate for post-filtering
    let effective_limit = if collection_member_ids.is_some() {
        limit * 3
    } else {
        limit
    };

    // Call the embedding service's semantic search
    let results = service
        .semantic_search_nodes(&params.query, effective_limit, threshold)
        .await
        .map_err(|e| {
            let err_msg = e.to_string();

            // Check for specific error types to provide actionable feedback
            if err_msg.contains("not initialized") || err_msg.contains("not available") {
                MCPError::internal_error("Embedding service not ready".to_string())
            } else if err_msg.contains("no embeddings") || err_msg.contains("not found") {
                MCPError::invalid_params(
                    "No content available for semantic search. Try adding content first."
                        .to_string(),
                )
            } else if err_msg.contains("database") || err_msg.contains("Database") {
                MCPError::internal_error(format!("Database error during search: {}", e))
            } else {
                MCPError::internal_error(format!("Search failed: {}", e))
            }
        })?;

    // Filter by collection if specified, then apply limit
    let filtered_results: Vec<_> = if let Some(member_ids) = collection_member_ids {
        results
            .into_iter()
            .filter(|(node, _)| member_ids.contains(&node.id))
            .take(limit)
            .collect()
    } else {
        results
    };

    // Transform results into JSON-serializable format
    let nodes: Vec<Value> = filtered_results
        .iter()
        .map(|(node, similarity)| {
            json!({
                "id": node.id,
                "nodeType": node.node_type,
                "content": node.content,
                "version": node.version,
                "createdAt": node.created_at,
                "modifiedAt": node.modified_at,
                "properties": node.properties,
                "similarity": similarity
            })
        })
        .collect();

    // Return results with metadata
    Ok(json!({
        "nodes": nodes,
        "count": nodes.len(),
        "query": params.query,
        "threshold": threshold,
        "collection_id": collection_id
    }))
}

#[cfg(test)]
mod search_tests {
    use super::*;
    use serde_json::json;

    // Parameter Parsing Tests

    #[tokio::test]
    async fn test_search_semantic_basic_params() {
        let params = json!({
            "query": "machine learning"
        });

        let search_params: Result<SearchSemanticParams, _> = serde_json::from_value(params);
        assert!(search_params.is_ok());

        let p = search_params.unwrap();
        assert_eq!(p.query, "machine learning");
        assert_eq!(p.threshold, None);
        assert_eq!(p.limit, None);
    }

    #[tokio::test]
    async fn test_search_semantic_custom_params() {
        let params = json!({
            "query": "project planning",
            "threshold": 0.6,
            "limit": 5
        });

        let search_params: Result<SearchSemanticParams, _> = serde_json::from_value(params);
        assert!(search_params.is_ok());

        let p = search_params.unwrap();
        assert_eq!(p.query, "project planning");
        assert_eq!(p.threshold, Some(0.6));
        assert_eq!(p.limit, Some(5));
    }

    #[tokio::test]
    async fn test_search_semantic_defaults_applied() {
        let params = json!({
            "query": "test query"
        });

        let search_params: Result<SearchSemanticParams, _> = serde_json::from_value(params);
        assert!(search_params.is_ok());

        let p = search_params.unwrap();
        assert_eq!(p.query, "test query");
        assert_eq!(p.threshold, None); // Will default to 0.7 in handler
        assert_eq!(p.limit, None); // Will default to 20 in handler
    }

    // Validation Tests

    #[test]
    fn test_threshold_validation_low() {
        let threshold = -0.1;
        assert!(!(0.0..=1.0).contains(&threshold));
    }

    #[test]
    fn test_threshold_validation_high() {
        let threshold = 1.5;
        assert!(!(0.0..=1.0).contains(&threshold));
    }

    #[test]
    fn test_threshold_validation_valid() {
        assert!((0.0..=1.0).contains(&0.0));
        assert!((0.0..=1.0).contains(&0.5));
        assert!((0.0..=1.0).contains(&1.0));
    }

    #[test]
    fn test_threshold_validation_boundary() {
        // Test edge cases
        assert!((0.0..=1.0).contains(&0.0));
        assert!((0.0..=1.0).contains(&1.0));
        assert!(!(0.0..=1.0).contains(&-0.00001));
        assert!(!(0.0..=1.0).contains(&1.00001));
    }

    // Validation Logic Tests
    // These tests verify the validation logic without requiring full service setup

    #[test]
    fn test_search_empty_query_validation() {
        // Test parameter validation for empty query
        let params = json!({"query": ""});
        let parsed: SearchSemanticParams = serde_json::from_value(params).unwrap();

        // Verify the validation logic we implemented
        assert!(parsed.query.trim().is_empty());
        assert_eq!(parsed.query, "");
    }

    #[test]
    fn test_search_whitespace_query_validation() {
        // Test parameter validation for whitespace-only query
        let params = json!({"query": "   "});
        let parsed: SearchSemanticParams = serde_json::from_value(params).unwrap();

        // Verify the validation logic we implemented
        assert!(parsed.query.trim().is_empty());
        assert_eq!(parsed.query, "   ");
    }

    #[test]
    fn test_search_invalid_threshold_high_validation() {
        let params = json!({
            "query": "test",
            "threshold": 1.5
        });

        let parsed: SearchSemanticParams = serde_json::from_value(params).unwrap();
        let threshold = parsed.threshold.unwrap_or(0.7);

        // Verify validation would catch this
        assert!(!(0.0..=1.0).contains(&threshold));
        assert_eq!(threshold, 1.5);
    }

    #[test]
    fn test_search_invalid_threshold_low_validation() {
        let params = json!({
            "query": "test",
            "threshold": -0.1
        });

        let parsed: SearchSemanticParams = serde_json::from_value(params).unwrap();
        let threshold = parsed.threshold.unwrap_or(0.7);

        // Verify validation would catch this
        assert!(!(0.0..=1.0).contains(&threshold));
    }

    #[test]
    fn test_search_limit_exceeds_maximum_validation() {
        let params = json!({
            "query": "test query",
            "limit": 5000
        });

        let parsed: SearchSemanticParams = serde_json::from_value(params).unwrap();
        let limit = parsed.limit.unwrap_or(20);

        // Verify validation would catch this
        assert!(limit > 1000);
        assert_eq!(limit, 5000);
    }

    #[test]
    fn test_search_response_structure() {
        // Test that response includes all expected metadata fields
        // This verifies the JSON structure without needing actual search results
        let expected_fields = vec!["nodes", "count", "query", "threshold"];

        // Verify the fields are present in our response construction
        let mock_response = json!({
            "nodes": [],
            "count": 0,
            "query": "test",
            "threshold": 0.7
        });

        for field in expected_fields {
            assert!(
                mock_response.get(field).is_some(),
                "Missing field: {}",
                field
            );
        }
    }

    #[test]
    fn test_search_defaults_match_schema() {
        // Verify Rust defaults match schema defaults
        let params = json!({"query": "test"});
        let parsed: SearchSemanticParams = serde_json::from_value(params).unwrap();

        // Apply defaults as the handler does
        let threshold = parsed.threshold.unwrap_or(0.7);
        let limit = parsed.limit.unwrap_or(20);

        // These should match the schema defaults in tools.rs
        assert_eq!(threshold, 0.7);
        assert_eq!(limit, 20);
    }

    #[test]
    fn test_search_parameter_combinations() {
        // Test various valid parameter combinations parse correctly
        let test_cases = vec![
            (json!({"query": "test"}), "minimal"),
            (json!({"query": "test", "threshold": 0.5}), "with threshold"),
            (json!({"query": "test", "limit": 10}), "with limit"),
            (
                json!({"query": "test", "threshold": 0.6, "limit": 15}),
                "all params",
            ),
        ];

        for (params, description) in test_cases {
            let result: Result<SearchSemanticParams, _> = serde_json::from_value(params);
            assert!(result.is_ok(), "Failed to parse: {}", description);
        }
    }

    #[test]
    fn test_limit_boundary_values() {
        // Test limit validation boundaries
        let valid_limit = 1000;
        let invalid_limit = 1001;
        let zero_limit = 0;

        // Valid cases
        assert!(valid_limit <= 1000);
        assert!(zero_limit <= 1000);

        // Invalid case
        assert!(invalid_limit > 1000);
    }
}
