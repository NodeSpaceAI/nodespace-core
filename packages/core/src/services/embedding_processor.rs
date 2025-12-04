//! Background Embedding Processor (Issue #729)
//!
//! Provides background processing of stale root-aggregate embeddings with:
//! - Periodic batch processing (30-second interval)
//! - Manual trigger support for explicit user actions
//! - Graceful shutdown with tokio::select!
//!
//! ## Root-Aggregate Model
//!
//! This processor works with the root-aggregate embedding model where:
//! - Only root nodes (no parent) of embeddable types get embedded
//! - Embeddings are stored in the `embedding` table, not on nodes
//! - The `stale` flag tracks which embeddings need regeneration

use crate::services::error::NodeServiceError;
use crate::services::NodeEmbeddingService;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;

/// Background interval for processing stale embeddings (30 seconds)
const BACKGROUND_INTERVAL_SECS: u64 = 30;

/// Embedding processor for background tasks
///
/// Processes stale embeddings in the background using the root-aggregate model.
pub struct EmbeddingProcessor {
    #[allow(dead_code)]
    embedding_service: Arc<NodeEmbeddingService>,
    trigger_tx: mpsc::Sender<()>,
    _shutdown_tx: mpsc::Sender<()>,
}

impl EmbeddingProcessor {
    /// Create and start embedding processor with background task
    ///
    /// Spawns a background task that periodically processes stale embeddings
    /// every 30 seconds. The task can be triggered manually via `trigger_batch_embed()`
    /// and will gracefully shutdown when the processor is dropped.
    ///
    /// ## Root-Aggregate Model (Issue #729)
    ///
    /// Uses `process_stale_embeddings()` which:
    /// 1. Queries the `embedding` table for stale entries
    /// 2. For each stale root node, aggregates its subtree content
    /// 3. Generates new embeddings and updates the table
    ///
    /// # Arguments
    /// * `embedding_service` - The embedding service for processing nodes
    ///
    /// # Returns
    /// A new EmbeddingProcessor instance with active background task
    pub fn new(embedding_service: Arc<NodeEmbeddingService>) -> Result<Self, NodeServiceError> {
        tracing::info!("EmbeddingProcessor initializing with root-aggregate model");

        let (trigger_tx, mut trigger_rx) = mpsc::channel(10);
        let (shutdown_tx, mut shutdown_rx) = mpsc::channel(1);

        // Spawn background task for periodic processing
        let service_clone = embedding_service.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(BACKGROUND_INTERVAL_SECS));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

            loop {
                tokio::select! {
                    // Periodic processing
                    _ = interval.tick() => {
                        tracing::debug!("Background embedding processor tick");
                        if let Err(e) = Self::process_batch(&service_clone).await {
                            tracing::error!("Background embedding processing failed: {}", e);
                        }
                    }

                    // Manual trigger
                    Some(_) = trigger_rx.recv() => {
                        tracing::info!("Manual embedding batch triggered");
                        if let Err(e) = Self::process_batch(&service_clone).await {
                            tracing::error!("Manual embedding processing failed: {}", e);
                        }
                    }

                    // Graceful shutdown
                    _ = shutdown_rx.recv() => {
                        tracing::info!("EmbeddingProcessor shutting down");
                        break;
                    }
                }
            }
        });

        Ok(Self {
            embedding_service,
            trigger_tx,
            _shutdown_tx: shutdown_tx,
        })
    }

    /// Internal helper to process a batch of stale embeddings
    ///
    /// Uses `None` for batch limit to process ALL stale entries in a single run.
    /// This is intentional for background processing: the task runs periodically
    /// (every 30 seconds), not per-request, so we want to catch up on the full
    /// backlog each time.
    async fn process_batch(service: &Arc<NodeEmbeddingService>) -> Result<usize, NodeServiceError> {
        match service.process_stale_embeddings(None).await {
            Ok(0) => {
                tracing::debug!("No stale embeddings to process");
                Ok(0)
            }
            Ok(count) => {
                tracing::info!("Processed {} stale embeddings", count);
                Ok(count)
            }
            Err(e) => {
                tracing::error!("Batch embedding failed: {}", e);
                Err(e)
            }
        }
    }

    /// Trigger batch embedding immediately
    ///
    /// Useful for explicit user actions like "Sync All" button or app startup.
    /// Sends a trigger signal to the background task to process stale embeddings.
    ///
    /// # Returns
    /// Ok(()) if trigger was sent successfully (does not wait for completion)
    ///
    /// # Errors
    /// Returns error if the background task has shut down
    pub async fn trigger_batch_embed(&self) -> Result<(), NodeServiceError> {
        self.trigger_tx.send(()).await.map_err(|_| {
            NodeServiceError::SerializationError("Background task has shut down".to_string())
        })?;

        tracing::debug!("Batch embedding trigger sent");
        Ok(())
    }

    /// Shutdown processor gracefully
    ///
    /// Sends shutdown signal to background task. The task will complete
    /// any in-progress operations and exit cleanly.
    pub fn shutdown(self) -> Result<(), NodeServiceError> {
        tracing::info!("Shutting down EmbeddingProcessor");
        // Channels will be dropped, signaling shutdown
        Ok(())
    }
}
