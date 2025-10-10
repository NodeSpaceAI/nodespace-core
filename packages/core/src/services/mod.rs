//! Business Services
//!
//! This module contains the core business logic services:
//!
//! - `NodeService` - CRUD operations and hierarchy management
//! - `TopicEmbeddingService` - Embedding generation and semantic search
//! - `EmbeddingProcessor` - Background task for processing stale topic embeddings
//! - `SchemaService` - Dynamic schema creation and validation (planned)
//! - `SearchService` - Semantic search and query operations (planned)
//!
//! Services coordinate between the database layer and application logic,
//! implementing business rules and orchestrating complex operations.

pub mod embedding_processor;
pub mod embedding_service;
pub mod error;
pub mod node_service;

pub use embedding_processor::{EmbeddingProcessor, EmbeddingProcessorConfig};
pub use embedding_service::{TopicEmbeddingService, EMBEDDING_DIMENSION};
pub use error::NodeServiceError;
pub use node_service::NodeService;
