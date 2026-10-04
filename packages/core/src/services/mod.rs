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

    /// The topmost non-person descendants of embedding root `root_id` that
    /// hold a `member_of` edge (ADR-059 §7, ADR-083 §5). Aggregation leaves
    /// each one, and its subtree, out of the root's vector. Empty whenever
    /// root-only membership holds.
    async fn access_boundaries_under(
        &self,
        root_id: &str,
    ) -> Result<std::collections::HashSet<String>, error::NodeServiceError>;

    /// `node_type`'s `extends` chain, nearest scope first, so a behaviour can
    /// treat a subtype as the type it extends (ADR-086 §5).
    async fn type_chain(&self, node_type: &str) -> Result<Vec<String>, error::NodeServiceError>;
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
    ///
    /// `properties` is stored namespaced under the node's own type (e.g.
    /// `{"task": {"status": "done"}}`, see
    /// `NodeService::normalize_flat_properties_to_namespace`), and under
    /// ADR-078 a field inherited from an ancestor schema lives in that
    /// ancestor's own bucket instead (see
    /// `NodeService::bucket_properties_by_owner`). `type_chain` is the
    /// node's own `extends` chain, nearest-first (e.g. `["issue", "task"]`
    /// for an issue extending task, or just `["task"]` for a type that
    /// extends nothing) — normally `NodeService::resolve_type_chain(node_type)`,
    /// pre-resolved once per distinct type by the caller rather than per row,
    /// since resolving it needs async store access this synchronous check
    /// can't do. Each `property_filters` key is looked up across every
    /// bucket in the chain via [`find_namespaced_property`], nearest first.
    pub fn matches(
        &self,
        node_type: &str,
        properties: &serde_json::Value,
        type_chain: &[String],
    ) -> bool {
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
                    match find_namespaced_property(properties, &[key.as_str()], type_chain) {
                        Some(actual) if actual == expected => {}
                        _ => return false,
                    }
                }
            }
        }

        true
    }
}

/// Resolve a bare property key's value from a node's namespaced properties by
/// walking a chain of namespace buckets, nearest first (ADR-078).
///
/// A field inherited from an ancestor schema is stored under that ancestor's
/// own bucket (`NodeService::bucket_properties_by_owner`), not the node's own
/// bucket, so a lookup that only checks `properties.get(node_type)` misses
/// every inherited field. `chain` is tried in the given order and the first
/// bucket holding the (possibly nested, via `path_segments`) path wins.
///
/// Shared by [`SearchNodeFilters::matches`] (flat equality filters) and
/// `NodeService::node_matches_property_filter` (JSONPath filters with
/// comparison operators) — the two `property_filters` implementations that
/// both need the same namespaced-bucket lookup, just applied to
/// differently-shaped filter inputs.
///
/// Falls back to a flat top-level lookup when no bucket holds the path.
/// Namespacing is the rule for an ordinary instance node, but two categories
/// are deliberately exempt (`NodeService::normalize_flat_properties_to_namespace`):
/// a `schema`-type node's own definition fields (`node_type == "schema"` is
/// special-cased out of namespacing entirely — its properties, e.g.
/// `isCore`, sit flat at the top level, the same shape as before this
/// namespace-aware lookup existed), and `_`-prefixed bookkeeping keys
/// (`_seed`, `_schema_version`), which always stay at a fixed,
/// type-independent top-level path on every node regardless of type. Neither
/// has a namespace bucket to be found in above, so without this fallback a
/// filter naming either would silently stop matching anything.
pub(crate) fn find_namespaced_property<'a>(
    properties: &'a serde_json::Value,
    path_segments: &[&str],
    chain: &[String],
) -> Option<&'a serde_json::Value> {
    for bucket in chain {
        let mut candidate = properties.get(bucket.as_str());
        for segment in path_segments {
            candidate = candidate.and_then(|v| v.get(*segment));
        }
        if candidate.is_some() {
            return candidate;
        }
    }

    let mut candidate = Some(properties);
    for segment in path_segments {
        candidate = candidate.and_then(|v| v.get(*segment));
    }
    candidate
}

/// Whether `filters` carries at least one `property_filters` key — used to
/// decide whether pre-resolving a `type_chain` for
/// [`SearchNodeFilters::matches`] is worth a store round trip at all.
/// `None`, or an empty object (`{}`, which trivially matches every node —
/// see `test_empty_property_object_passes_all`), both need no chain:
/// `matches`'s `property_filters` loop is a no-op either way, regardless of
/// what `type_chain` holds.
pub(crate) fn needs_property_filter_chain(filters: Option<&SearchNodeFilters>) -> bool {
    filters
        .and_then(|f| f.property_filters.as_ref())
        .and_then(|pf| pf.as_object())
        .is_some_and(|obj| !obj.is_empty())
}

/// Look up `node_type`'s pre-resolved chain in `type_chains`, falling back
/// to a single-element chain of just `node_type` (no inheritance) when it
/// wasn't pre-resolved — the same shape a type nothing extends would
/// resolve to anyway. Shared by every call site that batches `type_chains`
/// across a result set before calling [`SearchNodeFilters::matches`] per
/// node.
pub(crate) fn chain_for_type(
    type_chains: &std::collections::HashMap<String, Vec<String>>,
    node_type: &str,
) -> Vec<String> {
    type_chains
        .get(node_type)
        .cloned()
        .unwrap_or_else(|| vec![node_type.to_string()])
}

/// Resolve the `extends` chain (ADR-078) for each of `node_types`, nearest
/// first. Every chain is read from the type-ancestry table, the one resolved
/// form of the `extends` edges; a core type's chain needs no read. Callers
/// that pre-resolve chains for a batch of search results before filtering
/// (`NodeEmbeddingService::semantic_search_nodes`,
/// `ops::search_ops::resolve_type_chains_for_filters`) pass the distinct types
/// of the batch.
pub(crate) async fn resolve_type_chains_from_store<'a>(
    store: &crate::db::SqliteStore,
    node_types: impl IntoIterator<Item = &'a str>,
) -> Result<std::collections::HashMap<String, Vec<String>>, error::NodeServiceError> {
    let mut chains = std::collections::HashMap::new();
    for node_type in node_types {
        if chains.contains_key(node_type) {
            continue;
        }
        chains.insert(
            node_type.to_string(),
            resolve_type_chain_from_store(store, node_type).await?,
        );
    }
    Ok(chains)
}

/// Resolve a single node type's `extends` ancestor chain directly against
/// the store, nearest-first (ADR-078) — same semantics as
/// [`node_service::NodeService::resolve_type_chain`] (which delegates here),
/// for callers that hold a `SqliteStore` but not a `NodeService`.
pub(crate) async fn resolve_type_chain_from_store(
    store: &crate::db::SqliteStore,
    node_type: &str,
) -> Result<Vec<String>, error::NodeServiceError> {
    store.type_chain(node_type).await.map_err(|e| {
        error::NodeServiceError::query_failed(format!(
            "Failed to resolve the type chain of '{node_type}': {e}"
        ))
    })
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
    render_subtree_markdown, CompletenessResult, CreateNodeParams, CreatedRelationship,
    NewRelationship, NodeService, StoredEdge, SubtreeData, WriteVerificationFault,
    DEFAULT_QUERY_LIMIT,
};
pub use query_service::{
    FilterOperator, FilterType, QueryDefinition, QueryFilter, QueryService, RelationshipHop,
    RelationshipPath, RelativeDate, RelativeDateAnchor, SortConfig, SortDirection,
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
        let chain = vec!["task".to_string()];
        assert!(f.matches("task", &json!({}), &chain));
        assert!(f.matches("text", &json!({}), &["text".to_string()]));
        assert!(!f.matches("header", &json!({}), &["header".to_string()]));
    }

    #[test]
    fn test_empty_node_types_allows_all() {
        let f = SearchNodeFilters {
            node_types: Some(vec![]),
            property_filters: None,
        };
        assert!(f.matches("task", &json!({}), &["task".to_string()]));
        assert!(f.matches("anything", &json!({}), &["anything".to_string()]));
    }

    /// Realistic namespaced fixture: properties are bucketed under the
    /// node's own type (`normalize_flat_properties_to_namespace`), not flat
    /// at the top level. Both filtered fields are declared and stored on
    /// `task` itself, so a single-element chain (no inheritance involved)
    /// is enough to find them.
    #[test]
    fn test_property_all_match() {
        let f = SearchNodeFilters {
            node_types: None,
            property_filters: Some(json!({"status": "done", "priority": "high"})),
        };
        assert!(f.matches(
            "task",
            &json!({"task": {"status": "done", "priority": "high"}}),
            &["task".to_string()]
        ));
    }

    #[test]
    fn test_property_value_mismatch() {
        let f = SearchNodeFilters {
            node_types: None,
            property_filters: Some(json!({"status": "done"})),
        };
        assert!(!f.matches(
            "task",
            &json!({"task": {"status": "in-progress"}}),
            &["task".to_string()]
        ));
    }

    #[test]
    fn test_property_key_missing() {
        let f = SearchNodeFilters {
            node_types: None,
            property_filters: Some(json!({"status": "done"})),
        };
        assert!(!f.matches(
            "task",
            &json!({"task": {"priority": "high"}}),
            &["task".to_string()]
        ));
    }

    #[test]
    fn test_property_partial_match_fails() {
        // AND logic: all must match
        let f = SearchNodeFilters {
            node_types: None,
            property_filters: Some(json!({"status": "done", "priority": "high"})),
        };
        assert!(!f.matches(
            "task",
            &json!({"task": {"status": "done", "priority": "low"}}),
            &["task".to_string()]
        ));
    }

    #[test]
    fn test_combined_both_pass() {
        let f = SearchNodeFilters {
            node_types: Some(vec!["task".into()]),
            property_filters: Some(json!({"status": "done"})),
        };
        assert!(f.matches(
            "task",
            &json!({"task": {"status": "done"}}),
            &["task".to_string()]
        ));
    }

    #[test]
    fn test_combined_type_fails() {
        let f = SearchNodeFilters {
            node_types: Some(vec!["task".into()]),
            property_filters: Some(json!({"status": "done"})),
        };
        assert!(!f.matches(
            "text",
            &json!({"text": {"status": "done"}}),
            &["text".to_string()]
        ));
    }

    #[test]
    fn test_combined_property_fails() {
        let f = SearchNodeFilters {
            node_types: Some(vec!["task".into()]),
            property_filters: Some(json!({"status": "done"})),
        };
        assert!(!f.matches(
            "task",
            &json!({"task": {"status": "in-progress"}}),
            &["task".to_string()]
        ));
    }

    #[test]
    fn test_no_filters_passes_all() {
        let f = SearchNodeFilters::default();
        assert!(f.matches(
            "any-type",
            &json!({"any-type": {"any": "val"}}),
            &["any-type".to_string()]
        ));
    }

    #[test]
    fn test_empty_property_object_passes_all() {
        let f = SearchNodeFilters {
            node_types: None,
            property_filters: Some(json!({})),
        };
        assert!(f.matches("task", &json!({}), &["task".to_string()]));
        assert!(f.matches(
            "task",
            &json!({"task": {"status": "done"}}),
            &["task".to_string()]
        ));
    }

    // -- Regression coverage for the namespace/extends-chain bug --
    //
    // Before the fix, `matches` did a flat `properties.get(key)` on the top
    // level of `node.properties`, which is never where a schema-typed node's
    // fields actually live (`NodeService::normalize_flat_properties_to_namespace`
    // always buckets them under the node's own type, extending or not) — so
    // `property_filters` silently matched nothing for every ordinary
    // schema-typed node.

    /// Own-type field: a plain, unextended `task` node stores `status` under
    /// its own `task` bucket. This is the baseline case every schema-typed
    /// node hits, extends chain or not.
    #[test]
    fn test_own_type_field_matches_namespaced_bucket() {
        let f = SearchNodeFilters {
            node_types: None,
            property_filters: Some(json!({"status": "done"})),
        };
        let properties = json!({"task": {"status": "done"}});
        assert!(f.matches("task", &properties, &["task".to_string()]));
    }

    /// Inherited (extends-chain) field: an `issue` node extends `task` and
    /// does not redeclare `status`, so per
    /// `NodeService::bucket_properties_by_owner` (ADR-078) the value is
    /// stored in `task`'s bucket — the declaring ancestor's — not `issue`'s
    /// own. A filter on the bare field name must still find it by walking
    /// the node's own extends chain, nearest-first.
    #[test]
    fn test_inherited_field_found_in_ancestor_bucket() {
        let f = SearchNodeFilters {
            node_types: None,
            property_filters: Some(json!({"status": "done"})),
        };
        let properties = json!({
            "issue": {"severity": "high"},
            "task": {"status": "done"},
        });
        let chain = vec!["issue".to_string(), "task".to_string()];
        assert!(f.matches("issue", &properties, &chain));
    }

    /// Same inherited-field shape as above, but the stored value doesn't
    /// match the filter — the chain walk must find the real value in the
    /// ancestor bucket and compare it, not just report "found a bucket" and
    /// pass.
    #[test]
    fn test_inherited_field_non_matching_value_fails() {
        let f = SearchNodeFilters {
            node_types: None,
            property_filters: Some(json!({"status": "done"})),
        };
        let properties = json!({
            "issue": {"severity": "high"},
            "task": {"status": "in-progress"},
        });
        let chain = vec!["issue".to_string(), "task".to_string()];
        assert!(!f.matches("issue", &properties, &chain));
    }

    /// `find_namespaced_property` supports a multi-segment path (used by
    /// `NodeService::node_matches_property_filter`'s JSONPath filters, e.g.
    /// `$.field.subfield`) — `SearchNodeFilters::matches` never reaches this
    /// branch itself, since its flat `property_filters` keys are always a
    /// single bare field name, but the shared helper's nested-lookup
    /// behavior is exercised directly here rather than only through
    /// `query.rs`'s own tests.
    #[test]
    fn test_find_namespaced_property_resolves_a_nested_segment() {
        let properties = json!({
            "task": {"metadata": {"priority": "high"}},
        });
        let chain = vec!["task".to_string()];
        assert_eq!(
            find_namespaced_property(&properties, &["metadata", "priority"], &chain),
            Some(&json!("high"))
        );
        assert_eq!(
            find_namespaced_property(&properties, &["metadata", "missing"], &chain),
            None
        );
    }

    /// A `schema`-type node's own definition fields (`isCore`, etc.) are
    /// deliberately never namespaced (`node_type == "schema"` is exempted in
    /// `NodeService::normalize_flat_properties_to_namespace`'s caller), so
    /// they sit flat at the top level of `properties` — not under a
    /// `"schema"` bucket, which a schema node's properties never has. A
    /// chain of `["schema"]` (schema has no `extends` ancestry) finds
    /// nothing in any bucket; the flat top-level fallback must still find
    /// the field, matching how this lookup behaved before it became
    /// namespace-aware.
    #[test]
    fn test_flat_top_level_fallback_finds_schema_node_own_fields() {
        let properties = json!({"isCore": true, "name": "Task"});
        let chain = vec!["schema".to_string()];
        assert_eq!(
            find_namespaced_property(&properties, &["isCore"], &chain),
            Some(&json!(true))
        );
    }

    /// `_`-prefixed bookkeeping keys (`_seed`, `_schema_version`) always stay
    /// at a fixed, type-independent top-level path
    /// (`normalize_flat_properties_to_namespace` never namespaces them, on
    /// any node type), so they need the same flat fallback as a schema
    /// node's own fields, on an otherwise perfectly ordinary namespaced node.
    #[test]
    fn test_flat_top_level_fallback_finds_underscore_prefixed_bookkeeping_key() {
        let properties = json!({"task": {"status": "done"}, "_seed": "abc123"});
        let chain = vec!["task".to_string()];
        assert_eq!(
            find_namespaced_property(&properties, &["_seed"], &chain),
            Some(&json!("abc123"))
        );
    }

    /// The flat fallback must not paper over a genuinely absent field: a key
    /// that exists in neither a chain bucket nor at the top level still
    /// reports not-found.
    #[test]
    fn test_flat_top_level_fallback_does_not_invent_a_missing_field() {
        let properties = json!({"task": {"status": "done"}});
        let chain = vec!["task".to_string()];
        assert_eq!(
            find_namespaced_property(&properties, &["nonexistent"], &chain),
            None
        );
    }
}
