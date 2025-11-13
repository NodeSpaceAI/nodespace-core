//! Background Embedding Processor
//!
//! **STATUS: Database tracking complete, NLP integration pending**
//!
//! Database-level embedding staleness tracking implemented in Issue #481.
//! Background processing will be re-enabled in a future issue after NLP integration.
//!
//! **Future Work (separate issue):**
//! - Update to use Arc<SurrealStore>
//! - Implement background task processing with SurrealDB
//! - Re-enable embedding batch operations

use crate::services::error::NodeServiceError;
use nodespace_nlp_engine::EmbeddingService;
use std::sync::Arc;
use tokio::sync::mpsc;

/// Embedding processor for background tasks (temporarily disabled)
pub struct EmbeddingProcessor {
    #[allow(dead_code)]
    nlp_engine: Arc<EmbeddingService>,
    _shutdown_tx: mpsc::Sender<()>,
}

impl EmbeddingProcessor {
    /// Create and start embedding processor (returns stub)
    ///
    /// Database tracking ready - background processing pending future NLP integration
    pub fn new(nlp_engine: Arc<EmbeddingService>) -> Result<Self, NodeServiceError> {
        tracing::info!(
            "EmbeddingProcessor stubbed - background processing pending future NLP integration"
        );
        let (_shutdown_tx, _) = mpsc::channel(1);
        Ok(Self {
            nlp_engine,
            _shutdown_tx,
        })
    }

    /// Stub: Trigger batch embedding
    ///
    /// Database tracking ready - full implementation pending future issue
    pub fn trigger_batch_embed(&self) -> Result<(), NodeServiceError> {
        tracing::debug!("Background embedding stub - NLP integration pending");
        Ok(())
    }

    /// Stub: Shutdown processor
    pub fn shutdown(self) -> Result<(), NodeServiceError> {
        Ok(())
    }
}
