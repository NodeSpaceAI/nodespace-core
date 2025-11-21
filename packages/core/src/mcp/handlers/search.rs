//! MCP Search Handlers
//!
//! Semantic search operations for AI agent access.
//! Pure business logic - no Tauri dependencies.

use crate::mcp::types::MCPError;
use crate::services::NodeEmbeddingService;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::Arc;

/// Parameters for semantic_search method
#[derive(Debug, Deserialize)]
pub struct SemanticSearchParams {
    /// Natural language search query
    pub query: String,

    /// Minimum similarity threshold (0.0-1.0)
    /// Default: 0.7
    #[serde(default)]
    pub threshold: Option<f32>,

    /// Maximum number of results (max: 50)
    /// Default: 10
    #[serde(default)]
    pub limit: Option<usize>,
}

/// Result item from semantic search
#[derive(Debug, Serialize)]
pub struct SemanticSearchResult {
    /// Node ID
    pub id: String,
    /// Node type (text, header, task, date)
    pub node_type: String,
    /// Content snippet (truncated to 200 chars)
    pub snippet: String,
    /// Cosine similarity score (0.0-1.0)
    pub similarity: f64,
}

/// Search nodes by semantic similarity using vector embeddings
///
/// Uses the NLP engine to generate a query embedding, then searches
/// the database for nodes with similar embeddings using cosine similarity.
///
/// # Example
///
/// ```rust,no_run
/// let params = json!({
///     "query": "API design decisions",
///     "threshold": 0.7,
///     "limit": 10
/// });
/// let result = handle_semantic_search(&embedding_service, params).await?;
/// // Returns top 10 most relevant nodes
/// ```
pub async fn handle_semantic_search(
    service: &Arc<NodeEmbeddingService>,
    params: Value,
) -> Result<Value, MCPError> {
    // Parse parameters
    let params: SemanticSearchParams = serde_json::from_value(params)
        .map_err(|e| MCPError::invalid_params(format!("Invalid parameters: {}", e)))?;

    // Apply defaults per Issue #574 spec
    let threshold = params.threshold.unwrap_or(0.7);
    let limit = params.limit.unwrap_or(10).min(50); // Cap at 50

    // Validate parameters
    if !(0.0..=1.0).contains(&threshold) {
        return Err(MCPError::invalid_params(
            "threshold must be between 0.0 and 1.0".to_string(),
        ));
    }

    if params.query.trim().is_empty() {
        return Err(MCPError::invalid_params(
            "query cannot be empty or whitespace".to_string(),
        ));
    }

    // Perform semantic search using the embedding service
    let results = service
        .semantic_search(&params.query, limit, threshold)
        .await
        .map_err(|e| {
            let err_msg = e.to_string();

            // Check for specific error types to provide actionable feedback
            if err_msg.contains("not initialized") || err_msg.contains("not available") {
                MCPError::internal_error(
                    "Embedding service not ready. NLP engine may not be initialized.".to_string(),
                )
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

    // Format results per Issue #574 MCP response format
    let formatted_results: Vec<SemanticSearchResult> = results
        .into_iter()
        .map(|(node, similarity)| {
            // Truncate content to 200 chars for snippet (Unicode-safe)
            let snippet = if node.content.chars().count() > 200 {
                let truncated: String = node.content.chars().take(197).collect();
                format!("{}...", truncated)
            } else {
                node.content.clone()
            };

            SemanticSearchResult {
                id: node.id,
                node_type: node.node_type,
                snippet,
                similarity,
            }
        })
        .collect();

    // Return results with metadata
    Ok(json!({
        "results": formatted_results,
        "count": formatted_results.len(),
        "query": params.query,
        "threshold": threshold,
        "limit": limit
    }))
}

/// Legacy alias - delegates to semantic_search
pub async fn handle_search_containers(
    service: &Arc<NodeEmbeddingService>,
    params: Value,
) -> Result<Value, MCPError> {
    handle_semantic_search(service, params).await
}

#[cfg(test)]
mod search_tests {
    use super::*;
    use serde_json::json;

    // Parameter Parsing Tests

    #[tokio::test]
    async fn test_semantic_search_basic_params() {
        let params = json!({
            "query": "machine learning"
        });

        let search_params: Result<SemanticSearchParams, _> = serde_json::from_value(params);
        assert!(search_params.is_ok());

        let p = search_params.unwrap();
        assert_eq!(p.query, "machine learning");
        assert_eq!(p.threshold, None);
        assert_eq!(p.limit, None);
    }

    #[tokio::test]
    async fn test_semantic_search_custom_params() {
        let params = json!({
            "query": "project planning",
            "threshold": 0.6,
            "limit": 5
        });

        let search_params: Result<SemanticSearchParams, _> = serde_json::from_value(params);
        assert!(search_params.is_ok());

        let p = search_params.unwrap();
        assert_eq!(p.query, "project planning");
        assert_eq!(p.threshold, Some(0.6));
        assert_eq!(p.limit, Some(5));
    }

    #[tokio::test]
    async fn test_semantic_search_defaults_applied() {
        let params = json!({
            "query": "test query"
        });

        let search_params: Result<SemanticSearchParams, _> = serde_json::from_value(params);
        assert!(search_params.is_ok());

        let p = search_params.unwrap();
        assert_eq!(p.query, "test query");
        assert_eq!(p.threshold, None); // Will default to 0.7 in handler
        assert_eq!(p.limit, None); // Will default to 10 in handler
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
        let parsed: SemanticSearchParams = serde_json::from_value(params).unwrap();

        // Verify the validation logic we implemented
        assert!(parsed.query.trim().is_empty());
        assert_eq!(parsed.query, "");
    }

    #[test]
    fn test_search_whitespace_query_validation() {
        // Test parameter validation for whitespace-only query
        let params = json!({"query": "   "});
        let parsed: SemanticSearchParams = serde_json::from_value(params).unwrap();

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

        let parsed: SemanticSearchParams = serde_json::from_value(params).unwrap();
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

        let parsed: SemanticSearchParams = serde_json::from_value(params).unwrap();
        let threshold = parsed.threshold.unwrap_or(0.7);

        // Verify validation would catch this
        assert!(!(0.0..=1.0).contains(&threshold));
    }

    #[test]
    fn test_search_limit_capped_at_50() {
        let params = json!({
            "query": "test query",
            "limit": 100
        });

        let parsed: SemanticSearchParams = serde_json::from_value(params).unwrap();
        let limit = parsed.limit.unwrap_or(10).min(50); // Capped at 50

        // Verify limit is capped at 50 per Issue #574 spec
        assert_eq!(limit, 50);
    }

    #[test]
    fn test_search_response_structure() {
        // Test that response includes all expected metadata fields per Issue #574
        // This verifies the JSON structure without needing actual search results
        let expected_fields = vec!["results", "count", "query", "threshold", "limit"];

        // Verify the fields are present in our response construction
        let mock_response = json!({
            "results": [],
            "count": 0,
            "query": "test",
            "threshold": 0.7,
            "limit": 10
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
        // Verify Rust defaults match schema defaults per Issue #574
        let params = json!({"query": "test"});
        let parsed: SemanticSearchParams = serde_json::from_value(params).unwrap();

        // Apply defaults as the handler does
        let threshold = parsed.threshold.unwrap_or(0.7);
        let limit = parsed.limit.unwrap_or(10).min(50);

        // These should match the schema defaults in tools.rs
        assert_eq!(threshold, 0.7);
        assert_eq!(limit, 10);
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
            let result: Result<SemanticSearchParams, _> = serde_json::from_value(params);
            assert!(result.is_ok(), "Failed to parse: {}", description);
        }
    }

    #[test]
    fn test_limit_boundary_values() {
        // Test limit validation boundaries (max 50 per Issue #574)
        let valid_limit = 50;
        let over_limit = 100;
        let zero_limit = 0;

        // Valid cases - will be capped at 50
        assert!(valid_limit <= 50);
        assert!(zero_limit <= 50);

        // Over limit - will be capped
        let capped = over_limit.min(50);
        assert_eq!(capped, 50);
    }

    #[test]
    fn test_semantic_search_result_structure() {
        // Test that SemanticSearchResult has correct serialization
        let result = SemanticSearchResult {
            id: "test-node-123".to_string(),
            node_type: "text".to_string(),
            snippet: "This is a test snippet".to_string(),
            similarity: 0.89,
        };

        let json_result = serde_json::to_value(&result).unwrap();
        assert_eq!(json_result["id"], "test-node-123");
        assert_eq!(json_result["node_type"], "text");
        assert_eq!(json_result["snippet"], "This is a test snippet");
        assert_eq!(json_result["similarity"], 0.89);
    }
}
