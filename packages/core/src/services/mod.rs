//! Business Services
//!
//! This module contains the core business logic services:
//!
//! - `NodeService` - CRUD operations and hierarchy management
//! - `NodeEmbeddingService` - Embedding generation and semantic search (`nlp` feature)
//! - `EmbeddingProcessor` - Background task for processing stale root embeddings (`nlp` feature)
//! - `NodeAccessor` - Read-only trait for behavior-driven node access
//! - `QueryService` - Query execution with SQL translation
//! - `CollectionService` - Collection path parsing and membership management
//!
//! Schema nodes are managed via generic NodeService CRUD operations.
//! Validation is handled by SchemaNodeBehavior.
//!
//! Services coordinate between the database layer and application logic,
//! implementing business rules and orchestrating complex operations.

use crate::models::Node;
use async_trait::async_trait;

pub mod collection_service;
#[cfg(feature = "nlp")]
pub mod embedding_processor;
#[cfg(feature = "nlp")]
pub mod embedding_service;
pub mod error;
pub mod node_service;
pub mod query_service;

/// Read-only node accessor for behavior-driven content extraction
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
/// - **`NodeService` implements this**: Ensures all business rules (mentions, etc.) apply
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

    /// Descendants of embedding root `root_id` whose access differs from the
    /// root's (ADR-059 §7). Aggregation leaves each one, and its subtree, out
    /// of the root's vector. Empty whenever root-only membership holds.
    async fn access_boundaries_under(
        &self,
        root_id: &str,
    ) -> Result<std::collections::HashSet<String>, error::NodeServiceError>;
}

/// Scope for semantic search queries
///
/// Controls which node types are included in search results. Replaces the
/// previous `exclude_types` parameter approach — callers don't need to know
/// about every type; they just declare intent.
#[derive(Debug, Clone, PartialEq)]
pub enum SearchScope {
    /// Default: the user's knowledge — `KNOWLEDGE_CORE_TYPES` plus user-defined types
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

/// Service-layer filters for semantic search queries.
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

/// Explicit insertion position for hierarchy operations.
///
/// Replaces the overloaded `insert_after_node_id: Option<&str>` pattern where
/// `None` silently meant "beginning" for most callers but "end" for sync
/// callers (who had to compute `last_child_id` themselves). This enum makes
/// caller intent unambiguous at the type level.
///
/// The borrowed variant `InsertPosition<'_>` is used for function parameters.
/// For owned storage (e.g. `CreateNodeParams`), use `InsertPositionOwned`.
#[derive(Debug, Clone, PartialEq)]
pub enum InsertPosition<'a> {
    /// Insert at the beginning of the parent's children list.
    Beginning,
    /// Insert at the end of the parent's children list.
    End,
    /// Insert directly after the named sibling.
    ///
    /// If the sibling is not found in the parent's children, falls back to
    /// `End` (preserving today's `move_node` fallback semantic for unknown
    /// siblings).
    After(&'a str),
}

/// Owned version of [`InsertPosition`] for storage in structs.
#[derive(Debug, Clone, PartialEq)]
pub enum InsertPositionOwned {
    Beginning,
    End,
    After(String),
}

impl InsertPositionOwned {
    pub fn as_ref(&self) -> InsertPosition<'_> {
        match self {
            InsertPositionOwned::Beginning => InsertPosition::Beginning,
            InsertPositionOwned::End => InsertPosition::End,
            InsertPositionOwned::After(id) => InsertPosition::After(id.as_str()),
        }
    }
}

pub use collection_service::{
    build_path_string, normalize_collection_name, parse_collection_path, validate_collection_name,
    CollectionPath, CollectionSegment, CollectionService, ResolvedCollection, ResolvedPath,
    COLLECTION_PATH_DELIMITER, MAX_COLLECTION_DEPTH,
};
#[cfg(feature = "nlp")]
pub use embedding_processor::{
    EmbeddingProcessor, EmbeddingScheduler, EmbeddingWaker, SchedulerPermit,
};
#[cfg(feature = "nlp")]
pub use embedding_service::{NodeEmbeddingService, EMBEDDING_DIMENSION};
pub use error::NodeServiceError;
pub use node_service::{
    flatten_subtree_content, CompletenessResult, CreateNodeParams, NodeService, ReplacedEdge,
    SubtreeData, WriteVerificationFault, DEFAULT_QUERY_LIMIT,
};
pub use query_service::{
    FilterOperator, FilterType, QueryDefinition, QueryFilter, QueryService, RelationshipType,
    SortConfig, SortDirection,
};

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // Unit tests for SearchNodeFilters

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
