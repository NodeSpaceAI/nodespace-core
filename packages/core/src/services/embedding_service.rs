//! Node Embedding Service
//!
//! **STATUS: Database tracking complete (Issue #481), NLP integration pending (Issue #TBD)**
//!
//! ## What's Complete
//!
//! - ✅ Database-level staleness tracking (`mark_embedding_stale`, `get_nodes_with_stale_embeddings`)
//! - ✅ Embedding vector storage in SurrealDB (`update_embedding` method)
//! - ✅ Atomic update operations with version control
//! - ✅ Service stubs in place for seamless future integration
//!
//! ## What's Pending (Future Issue #TBD - NLP Integration)
//!
//! - ⏳ NLP engine integration for embedding generation
//! - ⏳ Background batch processing of stale embeddings
//! - ⏳ Vector similarity search implementation
//! - ⏳ Re-enable embedding tests with SurrealStore
//!
//! **Migration Path:**
//! When NLP integration is ready:
//! 1. Update constructor to take `Arc<SurrealStore>`
//! 2. Implement `embed_container()` using SurrealStore methods
//! 3. Use `get_nodes_with_stale_embeddings()` for batch processing
//! 4. Call `update_embedding()` to store generated vectors and clear stale flag
//! 5. Implement SurrealDB-native vector search

use crate::models::Node;
use crate::services::error::NodeServiceError;
use nodespace_nlp_engine::EmbeddingService;
use std::sync::Arc;

/// Embedding vector dimension for BAAI/bge-small-en-v1.5 model
pub const EMBEDDING_DIMENSION: usize = 384;

/// Node embedding service (temporarily disabled)
pub struct NodeEmbeddingService {
    /// NLP engine for generating embeddings
    #[allow(dead_code)]
    nlp_engine: Arc<EmbeddingService>,
}

impl NodeEmbeddingService {
    /// Create a new NodeEmbeddingService (temporarily returns stub)
    ///
    /// Database-level embedding staleness tracking is now implemented.
    /// Full embedding generation will be re-enabled in a future issue.
    pub fn new(nlp_engine: Arc<EmbeddingService>) -> Self {
        tracing::info!(
            "NodeEmbeddingService stubbed - embedding generation pending future NLP integration"
        );
        Self { nlp_engine }
    }

    /// Stub: Embed container
    ///
    /// Database tracking ready - full implementation pending future issue
    pub fn embed_container(&self, _node: &Node) -> Result<(), NodeServiceError> {
        tracing::debug!("Embedding generation stub - NLP integration pending");
        Ok(())
    }

    /// Stub: Batch embed containers
    ///
    /// Database tracking ready - full implementation pending future issue
    pub fn batch_embed_containers(&self, _limit: Option<usize>) -> Result<usize, NodeServiceError> {
        tracing::debug!("Batch embedding stub - NLP integration pending");
        Ok(0)
    }
}
