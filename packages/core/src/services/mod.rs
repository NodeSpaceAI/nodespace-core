//! Business Services
//!
//! This module contains the core business logic services:
//!
//! - `NodeService` - CRUD operations and hierarchy management
//! - `NodeEmbeddingService` - Embedding generation and semantic search
//! - `EmbeddingProcessor` - Background task for processing stale root embeddings
//! - `NodeAccessor` - Read-only trait for behavior-driven node access (Issue #1018)
//! - `SchemaTableManager` - DDL generation for schema-defined tables
//! - `MigrationRegistry` - Schema migration infrastructure (for future use)
//! - `InboundRelationshipCache` - Fast NLP discovery of inbound relationships
//! - `McpServerService` - MCP server lifecycle management (Issue #715)
//! - `QueryService` - Query execution with SQL translation (Issue #440)
//! - `CollectionService` - Collection path parsing and membership management (Issue #756)
//!
//! Schema nodes are managed via generic NodeService CRUD operations (Issue #690).
//! Validation is handled by SchemaNodeBehavior. DDL generation by SchemaTableManager.
//!
//! Services coordinate between the database layer and application logic,
//! implementing business rules and orchestrating complex operations.

use crate::models::Node;
use async_trait::async_trait;

pub mod collection_service;
pub mod embedding_processor;
pub mod embedding_service;
pub mod error;
pub mod mcp_server_service;
pub mod migration_registry;
pub mod migrations;
pub mod node_service;
pub mod query_service;
pub mod relationship_cache;
pub mod schema_table_manager;

/// Read-only node accessor for behavior-driven content extraction (Issue #1018)
///
/// This trait provides a minimal, read-only interface for `NodeBehavior` implementations
/// to access related nodes during content aggregation (e.g., fetching children for
/// text/header nodes that aggregate subtree content into their embedding).
///
/// ## Design Rationale
///
/// - **Read-only**: Behaviors cannot mutate through this interface
/// - **Minimal**: Only the methods behaviors actually need (no `get_nodes_in_subtree`)
/// - **Trait-based**: Enables mocking in tests without a real database
/// - **`NodeService` implements this**: Ensures all business rules (migrations, mentions) apply
///
/// ## Circular Dependency Prevention
///
/// ```text
/// NodeService -> EmbeddingWaker (lightweight mpsc channel, not the service)
/// NodeEmbeddingService -> NodeService (via NodeAccessor for reads)
/// ```
///
/// No circular reference. The waker pattern already breaks the cycle.
#[async_trait]
pub trait NodeAccessor: Send + Sync {
    /// Get a single node by ID
    async fn get_node(&self, id: &str) -> Result<Option<Node>, error::NodeServiceError>;

    /// Get direct children of a node, sorted by fractional order
    async fn get_children(&self, parent_id: &str) -> Result<Vec<Node>, error::NodeServiceError>;

    /// Get multiple nodes by IDs (batch)
    async fn get_nodes(&self, ids: &[&str]) -> Result<Vec<Node>, error::NodeServiceError>;
}

/// Scope for semantic search queries (Issue #1018)
///
/// Controls which node types are included in search results. Replaces the
/// previous `exclude_types` parameter approach — callers don't need to know
/// about every type; they just declare intent.
#[derive(Debug, Clone, PartialEq)]
pub enum SearchScope {
    /// Default: knowledge nodes (text, header, code-block, schema, table)
    Knowledge,
    /// Only conversation nodes (ai-chat)
    Conversations,
    /// All embeddable types
    Everything,
    /// Custom type filter
    Custom {
        include_types: Vec<String>,
        exclude_types: Vec<String>,
    },
}

/// Service-layer filters for semantic search queries (Issue #1059).
///
/// Allows callers to narrow `semantic_search_nodes` results by node type and/or
/// property values without duplicating filter logic in every caller.
/// Both fields are optional; when absent, no additional filtering is applied.
/// Multiple `property_filters` entries are combined with AND logic.
#[derive(Debug, Clone, Default)]
pub struct SearchNodeFilters {
    /// Restrict results to nodes whose `node_type` is in this list.
    /// An empty list is treated as no restriction (all types eligible).
    pub node_types: Option<Vec<String>>,

    /// Restrict results to nodes that contain all specified property key-value pairs.
    /// Values are compared with strict equality against `node.properties`.
    /// Must be a JSON object; non-object values are silently ignored.
    pub property_filters: Option<serde_json::Value>,
}

impl SearchNodeFilters {
    /// Returns `true` when neither filter is set (no-op filter).
    pub fn is_empty(&self) -> bool {
        self.node_types.is_none() && self.property_filters.is_none()
    }

    /// Returns `true` if the given node passes all active filters.
    pub fn matches(&self, node_type: &str, properties: &serde_json::Value) -> bool {
        // node_types filter — empty list treated as no restriction
        if let Some(ref allowed) = self.node_types {
            if !allowed.is_empty() && !allowed.iter().any(|t| t == node_type) {
                return false;
            }
        }

        // property_filters: all specified key-value pairs must match (AND logic)
        if let Some(ref pf) = self.property_filters {
            if let Some(filter_obj) = pf.as_object() {
                for (key, expected) in filter_obj {
                    match properties.get(key) {
                        Some(actual) if actual == expected => {}
                        _ => return false,
                    }
                }
            }
        }

        true
    }
}

pub use collection_service::{
    build_path_string, normalize_collection_name, parse_collection_path, validate_collection_name,
    CollectionPath, CollectionSegment, CollectionService, ResolvedCollection, ResolvedPath,
    COLLECTION_PATH_DELIMITER, MAX_COLLECTION_DEPTH,
};
pub use embedding_processor::{EmbeddingProcessor, EmbeddingWaker};
pub use embedding_service::{NodeEmbeddingService, EMBEDDING_DIMENSION};
pub use error::NodeServiceError;
pub use mcp_server_service::{default_mcp_port, McpResponseCallback, McpServerService};
pub use migration_registry::{MigrationRegistry, MigrationTransform};
pub use node_service::{CreateNodeParams, NodeService, SubtreeData, DEFAULT_QUERY_LIMIT};
pub use query_service::{
    FilterOperator, FilterType, QueryDefinition, QueryFilter, QueryService, RelationshipType,
    SortConfig, SortDirection,
};
pub use relationship_cache::{CacheStats, InboundRelationship, InboundRelationshipCache};
pub use schema_table_manager::SchemaTableManager;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // Unit tests for SearchNodeFilters (Issue #1059)

    #[test]
    fn test_default_is_empty() {
        assert!(SearchNodeFilters::default().is_empty());
    }

    #[test]
    fn test_with_node_types_is_not_empty() {
        let f = SearchNodeFilters {
            node_types: Some(vec!["task".into()]),
            property_filters: None,
        };
        assert!(!f.is_empty());
    }

    #[test]
    fn test_with_property_filters_is_not_empty() {
        let f = SearchNodeFilters {
            node_types: None,
            property_filters: Some(json!({"status": "done"})),
        };
        assert!(!f.is_empty());
    }

    #[test]
    fn test_matches_node_type_in_list() {
        let f = SearchNodeFilters {
            node_types: Some(vec!["task".into(), "text".into()]),
            property_filters: None,
        };
        assert!(f.matches("task", &json!({})));
        assert!(f.matches("text", &json!({})));
        assert!(!f.matches("header", &json!({})));
    }

    #[test]
    fn test_empty_node_types_allows_all() {
        let f = SearchNodeFilters {
            node_types: Some(vec![]),
            property_filters: None,
        };
        assert!(f.matches("task", &json!({})));
        assert!(f.matches("anything", &json!({})));
    }

    #[test]
    fn test_property_all_match() {
        let f = SearchNodeFilters {
            node_types: None,
            property_filters: Some(json!({"status": "done", "priority": "high"})),
        };
        assert!(f.matches("task", &json!({"status": "done", "priority": "high"})));
    }

    #[test]
    fn test_property_value_mismatch() {
        let f = SearchNodeFilters {
            node_types: None,
            property_filters: Some(json!({"status": "done"})),
        };
        assert!(!f.matches("task", &json!({"status": "in-progress"})));
    }

    #[test]
    fn test_property_key_missing() {
        let f = SearchNodeFilters {
            node_types: None,
            property_filters: Some(json!({"status": "done"})),
        };
        assert!(!f.matches("task", &json!({"priority": "high"})));
    }

    #[test]
    fn test_property_partial_match_fails() {
        // AND logic: all must match
        let f = SearchNodeFilters {
            node_types: None,
            property_filters: Some(json!({"status": "done", "priority": "high"})),
        };
        assert!(!f.matches("task", &json!({"status": "done", "priority": "low"})));
    }

    #[test]
    fn test_combined_both_pass() {
        let f = SearchNodeFilters {
            node_types: Some(vec!["task".into()]),
            property_filters: Some(json!({"status": "done"})),
        };
        assert!(f.matches("task", &json!({"status": "done"})));
    }

    #[test]
    fn test_combined_type_fails() {
        let f = SearchNodeFilters {
            node_types: Some(vec!["task".into()]),
            property_filters: Some(json!({"status": "done"})),
        };
        assert!(!f.matches("text", &json!({"status": "done"})));
    }

    #[test]
    fn test_combined_property_fails() {
        let f = SearchNodeFilters {
            node_types: Some(vec!["task".into()]),
            property_filters: Some(json!({"status": "done"})),
        };
        assert!(!f.matches("task", &json!({"status": "in-progress"})));
    }

    #[test]
    fn test_no_filters_passes_all() {
        let f = SearchNodeFilters::default();
        assert!(f.matches("any-type", &json!({"any": "val"})));
    }

    #[test]
    fn test_empty_property_object_passes_all() {
        let f = SearchNodeFilters {
            node_types: None,
            property_filters: Some(json!({})),
        };
        assert!(f.matches("task", &json!({})));
        assert!(f.matches("task", &json!({"status": "done"})));
    }
}
