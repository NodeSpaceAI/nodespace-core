//! Business Services
//!
//! This module contains the core business logic services:
//!
//! - `NodeService` - CRUD operations and hierarchy management
//! - `NodeEmbeddingService` - Embedding generation and semantic search
//! - `EmbeddingProcessor` - Background task for processing stale root embeddings
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
