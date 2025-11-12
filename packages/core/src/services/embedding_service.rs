//! Node Embedding Service
//!
//! **STATUS: TEMPORARILY DISABLED** (Issue #481)
//!
//! This module is temporarily stubbed out during the SurrealDB migration.
//! Embedding generation will be re-enabled after SurrealDB integration is complete.
//!
//! TODO(#481): Migrate embedding service to use SurrealStore instead of DatabaseService
//! - Update constructor to take Arc<SurrealStore>
//! - Migrate all database operations to SurrealStore methods
//! - Implement SurrealDB-native vector search
//! - Test embedding generation and search functionality

use crate::models::Node;
use crate::services::error::NodeServiceError;
use nodespace_nlp_engine::EmbeddingService;
use std::sync::Arc;

/// Embedding vector dimension for BAAI/bge-small-en-v1.5 model
pub const EMBEDDING_DIMENSION: usize = 384;

/// Node embedding service (temporarily disabled)
pub struct NodeEmbeddingService {
    /// NLP engine for generating embeddings
    nlp_engine: Arc<EmbeddingService>,
}

impl NodeEmbeddingService {
    /// Create a new NodeEmbeddingService (temporarily returns stub)
    ///
    /// TODO(#481): Re-enable once SurrealDB integration complete
    pub fn new(nlp_engine: Arc<EmbeddingService>) -> Self {
        tracing::warn!("NodeEmbeddingService temporarily disabled during SurrealDB migration (Issue #481)");
        Self { nlp_engine }
    }

    /// Stub: Embed container
    pub async fn embed_container(&self, _node: &Node) -> Result<(), NodeServiceError> {
        tracing::warn!("Embedding generation temporarily disabled (Issue #481)");
        Ok(())
    }

    /// Stub: Batch embed containers
    pub async fn batch_embed_containers(&self, _limit: Option<usize>) -> Result<usize, NodeServiceError> {
        tracing::warn!("Batch embedding temporarily disabled (Issue #481)");
        Ok(0)
    }
}
