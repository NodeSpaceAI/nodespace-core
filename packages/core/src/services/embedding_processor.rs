//! Background Embedding Processor
//!
//! **STATUS: Fully integrated with SurrealDB and NLP engine (Issue #489)**
//!
//! Provides background processing of stale embeddings with:
//! - Periodic batch processing (30-second interval)
//! - Manual trigger support for explicit user actions
//! - Graceful shutdown with tokio::select!
//! - Two-level embeddability support (Issue #573)

use crate::behaviors::NodeBehaviorRegistry;
use crate::services::error::NodeServiceError;
use crate::services::NodeEmbeddingService;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;

/// Background interval for processing stale embeddings (30 seconds)
const BACKGROUND_INTERVAL_SECS: u64 = 30;

/// Embedding processor for background tasks
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
    /// # Arguments
    /// * `embedding_service` - The embedding service for processing nodes
    ///
    /// # Returns
    /// A new EmbeddingProcessor instance with active background task
    pub fn new(embedding_service: Arc<NodeEmbeddingService>) -> Result<Self, NodeServiceError> {
        tracing::info!("EmbeddingProcessor initializing with background task");

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

    /// Create and start embedding processor with two-level embeddability support
    ///
    /// Spawns a background task that periodically processes stale embeddings
    /// every 30 seconds using behavior-aware embedding. The task can be triggered
    /// manually via `trigger_batch_embed()` and will gracefully shutdown when
    /// the processor is dropped.
    ///
    /// # Two-Level Embeddability (Issue #573)
    ///
    /// This constructor uses the NodeBehaviorRegistry to determine:
    /// 1. Which nodes should be embedded (e.g., text/header yes, task/date no)
    /// 2. What content children contribute to parent embeddings
    ///
    /// # Arguments
    /// * `embedding_service` - The embedding service for processing nodes
    /// * `registry` - The behavior registry for looking up node type behaviors
    ///
    /// # Returns
    /// A new EmbeddingProcessor instance with active background task
    pub fn new_with_behavior(
        embedding_service: Arc<NodeEmbeddingService>,
        registry: Arc<NodeBehaviorRegistry>,
    ) -> Result<Self, NodeServiceError> {
        tracing::info!("EmbeddingProcessor initializing with two-level embeddability");

        let (trigger_tx, mut trigger_rx) = mpsc::channel(10);
        let (shutdown_tx, mut shutdown_rx) = mpsc::channel(1);

        // Spawn background task for periodic processing with behavior support
        let service_clone = embedding_service.clone();
        let registry_clone = registry;
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(BACKGROUND_INTERVAL_SECS));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

            loop {
                tokio::select! {
                    // Periodic processing with behavior
                    _ = interval.tick() => {
                        tracing::debug!("Background embedding processor tick (with behavior)");
                        if let Err(e) = Self::process_batch_with_behavior(&service_clone, &registry_clone).await {
                            tracing::error!("Background embedding processing failed: {}", e);
                        }
                    }

                    // Manual trigger with behavior
                    Some(_) = trigger_rx.recv() => {
                        tracing::info!("Manual embedding batch triggered (with behavior)");
                        if let Err(e) = Self::process_batch_with_behavior(&service_clone, &registry_clone).await {
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
    /// Uses `None` for batch limit to process ALL stale nodes in a single run.
    /// This is intentional for background processing: the task runs periodically
    /// (every 30 seconds), not per-request, so we want to catch up on the full
    /// backlog each time. For bounded processing, callers should use the service
    /// directly with an explicit limit.
    async fn process_batch(service: &Arc<NodeEmbeddingService>) -> Result<usize, NodeServiceError> {
        match service.batch_embed_containers(None).await {
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

    /// Internal helper to process a batch of stale embeddings with behavior support
    ///
    /// Uses two-level embeddability (Issue #573) to determine which nodes to embed
    /// and what content to include from children.
    async fn process_batch_with_behavior(
        service: &Arc<NodeEmbeddingService>,
        registry: &Arc<NodeBehaviorRegistry>,
    ) -> Result<usize, NodeServiceError> {
        match service
            .batch_embed_containers_with_behavior(registry, None)
            .await
        {
            Ok(0) => {
                tracing::debug!("No stale embeddings to process (with behavior)");
                Ok(0)
            }
            Ok(count) => {
                tracing::info!("Processed {} stale embeddings (with behavior)", count);
                Ok(count)
            }
            Err(e) => {
                tracing::error!("Batch embedding with behavior failed: {}", e);
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
