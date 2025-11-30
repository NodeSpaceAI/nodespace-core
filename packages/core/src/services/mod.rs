//! Business Services
//!
//! This module contains the core business logic services:
//!
//! - `NodeService` - CRUD operations and hierarchy management
//! - `NodeEmbeddingService` - Embedding generation and semantic search
//! - `EmbeddingProcessor` - Background task for processing stale root embeddings
//! - `SchemaTableManager` - DDL generation for schema-defined tables
//! - `MigrationRegistry` - Schema migration infrastructure (for future use)
//! - `SearchService` - Semantic search and query operations (planned)
//!
//! Schema nodes are managed via generic NodeService CRUD operations (Issue #690).
//! Validation is handled by SchemaNodeBehavior. DDL generation by SchemaTableManager.
//!
//! Services coordinate between the database layer and application logic,
//! implementing business rules and orchestrating complex operations.

pub mod embedding_processor;
pub mod embedding_service;
pub mod error;
pub mod migration_registry;
pub mod migrations;
pub mod node_service;
pub mod schema_table_manager;

#[cfg(test)]
mod node_service_root_test;

pub use embedding_processor::EmbeddingProcessor;
pub use embedding_service::{NodeEmbeddingService, EMBEDDING_DIMENSION};
pub use error::NodeServiceError;
pub use migration_registry::{MigrationRegistry, MigrationTransform};
pub use node_service::{CreateNodeParams, NodeService};
pub use schema_table_manager::SchemaTableManager;
