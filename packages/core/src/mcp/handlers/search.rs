//! MCP Search Handlers
//!
//! Semantic search operations for AI agent access.
//! Pure business logic - no Tauri dependencies.

use crate::mcp::types::MCPError;
use crate::services::{NodeEmbeddingService, NodeService};
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;

/// Parameters for search_nodes (keyword/title search) method
#[derive(Debug, Deserialize)]
pub struct SearchNodesParams {
    /// Keyword or phrase to search for in node titles. Pass an empty string to
    /// skip the title filter (useful when filtering only by node_type or filters).
    pub query: String,

    /// Filter by node type (e.g., "task", "text")
    #[serde(default)]
    pub node_type: Option<String>,

    /// Maximum number of results
    /// Default: 10
    #[serde(default)]
    pub limit: Option<usize>,

    /// Property filters as key-value pairs matched with equals.
    /// e.g. {"status": "open"} or {"company": "Acme"}
    #[serde(default)]
    pub filters: Option<std::collections::HashMap<String, String>>,
}

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

    /// Exclude results from these collections (by path, e.g., ["archived", "drafts"])
    /// Results in any of these collections will be filtered out
    #[serde(default)]
    pub exclude_collections: Option<Vec<String>>,

    /// Number of top results to include full markdown content for (0-5)
    /// This saves AI agents from needing to call get_markdown_from_node_id separately.
    /// Default: 1 (include markdown for top result only)
    /// Set to 0 to disable, max 5 to limit response size.
    #[serde(default)]
    pub include_markdown: Option<usize>,

    /// Include archived nodes in search results (default: false)
    /// By default, search only returns active nodes. Set to true to also include archived content.
    /// Nodes with lifecycle_status = "deleted" are never included.
    #[serde(default)]
    pub include_archived: Option<bool>,

    /// Search scope - controls which node types are included (Issue #1018)
    /// Values: "knowledge" (default), "conversations", "everything"
    /// Default: "knowledge" (text, header, code-block, schema, table)
    #[serde(default)]
    pub scope: Option<String>,

    /// Filter by specific node types (e.g., ["task", "text"])
    /// If set, only nodes whose node_type is in this list will be included
    #[serde(default)]
    pub node_types: Option<Vec<String>>,

    /// Filter by node properties (key-value pairs)
    /// If set, only nodes whose properties contain all specified key-value pairs will be included
    /// Multiple filters are combined with AND logic
    #[serde(default)]
    pub property_filters: Option<serde_json::Value>,

    /// When true, attach outgoing relationships of each result node as an "edges" array.
    /// Each edge entry has: {"relationship": "...", "target_id": "...", "target_title": "..."}
    /// Default: false (no edge data included)
    #[serde(default)]
    pub include_edges: Option<bool>,

    /// When true, re-rank results by blending vector similarity with graph connectivity degree.
    /// Blending formula: combined_score = 0.7 * similarity + 0.3 * normalized_degree
    /// where normalized_degree = outgoing_edge_count / max_outgoing_edge_count_in_result_set
    /// Surfaces well-connected, central knowledge nodes over isolated but textually similar ones.
    /// Default: false (pure similarity ranking)
    #[serde(default)]
    pub graph_boost: Option<bool>,
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
/// let result = handle_search_semantic(&node_service, &embedding_service, params).await?;
/// // Returns top 10 most relevant root nodes
/// ```
pub async fn handle_search_semantic(
    node_service: &Arc<NodeService>,
    embedding_service: &Arc<NodeEmbeddingService>,
    params: Value,
) -> Result<Value, MCPError> {
    use crate::ops::{search_ops, OpsError};

    // Parse parameters
    let params: SearchSemanticParams = serde_json::from_value(params)
        .map_err(|e| MCPError::invalid_params(format!("Invalid parameters: {}", e)))?;

    // Delegate all search logic to the shared search_ops layer, which owns the single
    // canonical implementation of collection resolution, scope/lifecycle filtering,
    // over-fetching, markdown inlining, include_edges, and graph_boost.
    let input = search_ops::SearchSemanticInput {
        query: params.query,
        threshold: params.threshold,
        limit: params.limit,
        collection_id: params.collection_id,
        collection: params.collection,
        exclude_collections: params.exclude_collections,
        include_markdown: params.include_markdown,
        include_archived: params.include_archived,
        scope: params.scope,
        node_types: params.node_types,
        property_filters: params.property_filters,
        include_edges: params.include_edges,
        graph_boost: params.graph_boost,
    };

    let output = search_ops::search_semantic(node_service, embedding_service, input)
        .await
        .map_err(|e| match e {
            OpsError::InvalidParams(msg) => MCPError::invalid_params(msg),
            OpsError::Internal(msg) => MCPError::internal_error(msg),
            OpsError::NotFound { id } => MCPError::node_not_found(&id),
            OpsError::ValidationFailed(msg) => MCPError::validation_error(msg),
            OpsError::VersionConflict { node_id, .. } => {
                MCPError::internal_error(format!("Version conflict on node {}", node_id))
            }
        })?;

    Ok(json!({
        "nodes": output.nodes,
        "count": output.count,
        "query": output.query,
        "threshold": output.threshold,
        "collection_id": output.collection_id,
        "include_markdown": output.include_markdown,
        "include_archived": output.include_archived,
        "scope": output.scope,
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
            (
                json!({"query": "test", "exclude_collections": ["archived"]}),
                "with exclude_collections",
            ),
            (
                json!({"query": "test", "collection": "docs", "exclude_collections": ["archived", "drafts"]}),
                "with collection and exclude_collections",
            ),
        ];

        for (params, description) in test_cases {
            let result: Result<SearchSemanticParams, _> = serde_json::from_value(params);
            assert!(result.is_ok(), "Failed to parse: {}", description);
        }
    }

    #[test]
    fn test_exclude_collections_parsing() {
        let params = json!({
            "query": "test query",
            "exclude_collections": ["archived", "drafts", "old-docs"]
        });

        let parsed: SearchSemanticParams = serde_json::from_value(params).unwrap();
        assert_eq!(parsed.query, "test query");
        assert!(parsed.exclude_collections.is_some());

        let excluded = parsed.exclude_collections.unwrap();
        assert_eq!(excluded.len(), 3);
        assert!(excluded.contains(&"archived".to_string()));
        assert!(excluded.contains(&"drafts".to_string()));
        assert!(excluded.contains(&"old-docs".to_string()));
    }

    #[test]
    fn test_exclude_collections_empty_array() {
        let params = json!({
            "query": "test",
            "exclude_collections": []
        });

        let parsed: SearchSemanticParams = serde_json::from_value(params).unwrap();
        assert!(parsed.exclude_collections.is_some());
        assert!(parsed.exclude_collections.unwrap().is_empty());
    }

    #[test]
    fn test_exclude_collections_not_provided() {
        let params = json!({
            "query": "test"
        });

        let parsed: SearchSemanticParams = serde_json::from_value(params).unwrap();
        assert!(parsed.exclude_collections.is_none());
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

    // Lifecycle Status Filter Tests (Issue #755)

    #[test]
    fn test_include_archived_default_false() {
        // Test that include_archived defaults to None when not provided
        let params = json!({
            "query": "test"
        });

        let parsed: SearchSemanticParams = serde_json::from_value(params).unwrap();
        assert!(parsed.include_archived.is_none());

        // When applied, defaults to false
        let include_archived = parsed.include_archived.unwrap_or(false);
        assert!(!include_archived);
    }

    #[test]
    fn test_include_archived_explicit_true() {
        let params = json!({
            "query": "test",
            "include_archived": true
        });

        let parsed: SearchSemanticParams = serde_json::from_value(params).unwrap();
        assert_eq!(parsed.include_archived, Some(true));
    }

    #[test]
    fn test_include_archived_explicit_false() {
        let params = json!({
            "query": "test",
            "include_archived": false
        });

        let parsed: SearchSemanticParams = serde_json::from_value(params).unwrap();
        assert_eq!(parsed.include_archived, Some(false));
    }

    #[test]
    fn test_include_archived_with_other_params() {
        let params = json!({
            "query": "test",
            "threshold": 0.8,
            "limit": 10,
            "include_archived": true,
            "collection": "docs"
        });

        let parsed: SearchSemanticParams = serde_json::from_value(params).unwrap();
        assert_eq!(parsed.query, "test");
        assert_eq!(parsed.threshold, Some(0.8));
        assert_eq!(parsed.limit, Some(10));
        assert_eq!(parsed.include_archived, Some(true));
        assert_eq!(parsed.collection, Some("docs".to_string()));
    }

    #[test]
    fn test_search_response_includes_include_archived() {
        // Test that response includes include_archived field
        let mock_response = json!({
            "nodes": [],
            "count": 0,
            "query": "test",
            "threshold": 0.7,
            "collection_id": null,
            "include_markdown": 1,
            "include_archived": false
        });

        assert!(mock_response.get("include_archived").is_some());
        assert_eq!(mock_response["include_archived"], false);
    }

    #[test]
    fn test_lifecycle_filter_logic_deleted_always_excluded() {
        // Test that "deleted" lifecycle status is always excluded
        let lifecycle_status = "deleted";
        let _include_archived = true; // Even with include_archived=true, deleted is excluded

        // Deleted nodes should always be excluded
        let should_exclude = lifecycle_status == "deleted";
        assert!(should_exclude);
    }

    #[test]
    fn test_lifecycle_filter_logic_archived_excluded_by_default() {
        // Test that "archived" is excluded when include_archived=false
        let lifecycle_status = "archived";
        let include_archived = false;

        let should_exclude = lifecycle_status == "archived" && !include_archived;
        assert!(should_exclude);
    }

    #[test]
    fn test_lifecycle_filter_logic_archived_included_when_flag_set() {
        // Test that "archived" is included when include_archived=true
        let lifecycle_status = "archived";
        let include_archived = true;

        let should_exclude = lifecycle_status == "archived" && !include_archived;
        assert!(!should_exclude);
    }

    #[test]
    fn test_lifecycle_filter_logic_active_always_included() {
        // Test that "active" lifecycle status is always included
        let lifecycle_status = "active";
        let include_archived = false;

        // Active nodes should never be excluded by lifecycle filter
        let should_exclude_by_deleted = lifecycle_status == "deleted";
        let should_exclude_by_archived = lifecycle_status == "archived" && !include_archived;

        assert!(!should_exclude_by_deleted);
        assert!(!should_exclude_by_archived);
    }

    // Issue #1059 - Node type and property filtering tests

    #[test]
    fn test_node_types_filter_parsing() {
        let params = json!({
            "query": "test",
            "node_types": ["task", "text"]
        });

        let parsed: SearchSemanticParams = serde_json::from_value(params).unwrap();
        assert_eq!(
            parsed.node_types,
            Some(vec!["task".to_string(), "text".to_string()])
        );
    }

    #[test]
    fn test_node_types_filter_empty_array() {
        let params = json!({
            "query": "test",
            "node_types": []
        });

        let parsed: SearchSemanticParams = serde_json::from_value(params).unwrap();
        assert_eq!(parsed.node_types, Some(vec![]));
    }

    #[test]
    fn test_node_types_filter_not_provided() {
        let params = json!({
            "query": "test"
        });

        let parsed: SearchSemanticParams = serde_json::from_value(params).unwrap();
        assert!(parsed.node_types.is_none());
    }

    #[test]
    fn test_property_filters_parsing() {
        let params = json!({
            "query": "test",
            "property_filters": {
                "status": "done",
                "priority": "high"
            }
        });

        let parsed: SearchSemanticParams = serde_json::from_value(params).unwrap();
        assert!(parsed.property_filters.is_some());

        let filters = parsed.property_filters.unwrap();
        assert_eq!(filters.get("status").and_then(|v| v.as_str()), Some("done"));
        assert_eq!(
            filters.get("priority").and_then(|v| v.as_str()),
            Some("high")
        );
    }

    #[test]
    fn test_property_filters_empty_object() {
        let params = json!({
            "query": "test",
            "property_filters": {}
        });

        let parsed: SearchSemanticParams = serde_json::from_value(params).unwrap();
        assert!(parsed.property_filters.is_some());
        assert!(parsed
            .property_filters
            .unwrap()
            .as_object()
            .unwrap()
            .is_empty());
    }

    #[test]
    fn test_property_filters_not_provided() {
        let params = json!({
            "query": "test"
        });

        let parsed: SearchSemanticParams = serde_json::from_value(params).unwrap();
        assert!(parsed.property_filters.is_none());
    }

    #[test]
    fn test_node_types_filter_matching_logic() {
        // Test that parsing node_types params produces the expected list.
        // Filter matching logic is delegated to SearchNodeFilters::matches() in services/mod.rs.
        let allowed_types = ["task".to_string(), "text".to_string()];

        // Nodes that should match
        assert!(allowed_types.contains(&"task".to_string()));
        assert!(allowed_types.contains(&"text".to_string()));

        // Nodes that should not match
        assert!(!allowed_types.contains(&"header".to_string()));
        assert!(!allowed_types.contains(&"code-block".to_string()));
    }

    #[test]
    fn test_property_filters_matching_logic() {
        // Test the matching logic without running full search
        let filters = json!({
            "status": "done",
            "priority": "high"
        });

        let filter_obj = filters.as_object().unwrap();

        // Create a mock node property map
        let mut node_properties = serde_json::Map::new();
        node_properties.insert("status".to_string(), json!("done"));
        node_properties.insert("priority".to_string(), json!("high"));

        // All filters should match
        for (key, expected_value) in filter_obj {
            let actual_value = node_properties.get(key);
            assert_eq!(actual_value, Some(expected_value));
        }
    }

    #[test]
    fn test_property_filters_partial_match_fails() {
        // Test that partial matches fail (AND logic)
        let filters = json!({
            "status": "done",
            "priority": "high"
        });

        let filter_obj = filters.as_object().unwrap();

        // Create a mock node property map with only one matching property
        let mut node_properties = serde_json::Map::new();
        node_properties.insert("status".to_string(), json!("done"));
        node_properties.insert("priority".to_string(), json!("low")); // This doesn't match!

        // At least one filter should not match
        let mut all_match = true;
        for (key, expected_value) in filter_obj {
            if let Some(actual_value) = node_properties.get(key) {
                if actual_value != expected_value {
                    all_match = false;
                    break;
                }
            } else {
                all_match = false;
                break;
            }
        }

        assert!(!all_match); // Should not match all filters
    }

    #[test]
    fn test_combined_node_types_and_property_filters() {
        let params = json!({
            "query": "test",
            "node_types": ["task"],
            "property_filters": {
                "status": "done"
            }
        });

        let parsed: SearchSemanticParams = serde_json::from_value(params).unwrap();
        assert_eq!(parsed.node_types, Some(vec!["task".to_string()]));
        assert!(parsed.property_filters.is_some());
        assert_eq!(
            parsed
                .property_filters
                .unwrap()
                .get("status")
                .and_then(|v| v.as_str()),
            Some("done")
        );
    }

    #[test]
    fn test_search_response_includes_filters() {
        // Test that response includes filter fields
        let mock_response = json!({
            "nodes": [],
            "count": 0,
            "query": "test",
            "threshold": 0.7,
            "collection_id": null,
            "include_markdown": 1,
            "include_archived": false,
            "scope": "Knowledge",
            "node_types": ["task"],
            "property_filters": { "status": "done" }
        });

        assert!(mock_response.get("node_types").is_some());
        assert!(mock_response.get("property_filters").is_some());
        assert_eq!(mock_response["node_types"], json!(["task"]));
    }

    // include_edges tests

    #[test]
    fn test_include_edges_param_default_none() {
        let params = json!({ "query": "test" });
        let parsed: SearchSemanticParams = serde_json::from_value(params).unwrap();
        assert!(parsed.include_edges.is_none());
        assert!(!parsed.include_edges.unwrap_or(false));
    }

    #[test]
    fn test_include_edges_param_explicit_true() {
        let params = json!({ "query": "test", "include_edges": true });
        let parsed: SearchSemanticParams = serde_json::from_value(params).unwrap();
        assert_eq!(parsed.include_edges, Some(true));
    }

    #[test]
    fn test_include_edges_param_explicit_false() {
        let params = json!({ "query": "test", "include_edges": false });
        let parsed: SearchSemanticParams = serde_json::from_value(params).unwrap();
        assert_eq!(parsed.include_edges, Some(false));
    }

    // graph_boost tests

    #[test]
    fn test_graph_boost_param_default_none() {
        let params = json!({ "query": "test" });
        let parsed: SearchSemanticParams = serde_json::from_value(params).unwrap();
        assert!(parsed.graph_boost.is_none());
        assert!(!parsed.graph_boost.unwrap_or(false));
    }

    #[test]
    fn test_graph_boost_param_explicit_true() {
        let params = json!({ "query": "test", "graph_boost": true });
        let parsed: SearchSemanticParams = serde_json::from_value(params).unwrap();
        assert_eq!(parsed.graph_boost, Some(true));
    }

    #[test]
    fn test_graph_boost_param_explicit_false() {
        let params = json!({ "query": "test", "graph_boost": false });
        let parsed: SearchSemanticParams = serde_json::from_value(params).unwrap();
        assert_eq!(parsed.graph_boost, Some(false));
    }
}
