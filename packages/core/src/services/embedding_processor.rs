//! Background Embedding Processor
//!
//! **STATUS: TEMPORARILY DISABLED** (Issue #481)
//!
//! This module is temporarily stubbed out during the SurrealDB migration.
//! Background embedding processing will be re-enabled after SurrealDB integration is complete.
//!
//! TODO(#481): Migrate embedding processor to use SurrealStore
//! - Update to use Arc<SurrealStore>
//! - Implement background task processing with SurrealDB
//! - Re-enable embedding batch operations

use crate::services::error::NodeServiceError;
use nodespace_nlp_engine::EmbeddingService;
use std::sync::Arc;
use tokio::sync::mpsc;

/// Embedding processor for background tasks (temporarily disabled)
pub struct EmbeddingProcessor {
    nlp_engine: Arc<EmbeddingService>,
    _shutdown_tx: mpsc::Sender<()>,
}

impl EmbeddingProcessor {
    /// Create and start embedding processor (returns stub)
    ///
    /// TODO(#481): Re-enable background embedding processing
    pub fn new(nlp_engine: Arc<EmbeddingService>) -> Result<Self, NodeServiceError> {
        tracing::warn!(
            "EmbeddingProcessor temporarily disabled during SurrealDB migration (Issue #481)"
        );
        let (_shutdown_tx, _) = mpsc::channel(1);
        Ok(Self {
            nlp_engine,
            _shutdown_tx,
        })
    }

    /// Stub: Trigger batch embedding
    pub fn trigger_batch_embed(&self) -> Result<(), NodeServiceError> {
        tracing::warn!("Background embedding temporarily disabled (Issue #481)");
        Ok(())
    }

    /// Stub: Shutdown processor
    pub fn shutdown(self) -> Result<(), NodeServiceError> {
        Ok(())
    }
}
