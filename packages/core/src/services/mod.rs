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
