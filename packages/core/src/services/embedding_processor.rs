//! Background Embedding Processor (Issue #729)
//!
//! Provides event-driven background processing of stale root-aggregate embeddings:
//! - Purely event-driven: only wakes when nodes change
//! - Per-root debounce: each root waits 30s after last change before embedding
//! - Processes embeddings that have passed their debounce window
//! - Graceful shutdown support
//!
//! ## Event-Driven Model with Per-Root Debounce
//!
//! The processor is purely event-driven with smart debounce scheduling:
//! 1. Woken by triggers when nodes change (marks stale, resets debounce timer)
//! 2. When woken, processes any embeddings that have passed their debounce window
//! 3. If stale embeddings exist but haven't passed debounce, schedules a delayed wake
//! 4. Zero CPU overhead when idle - no polling
//!
//! This ensures:
//! - Rapid edits don't trigger constant re-embedding (debounce per root)
//! - Bulk imports wait until complete before embedding (all children created)
//! - Independent documents don't block each other (per-root timers)
//! - No wasted cycles polling when there's no work
//!
//! ## Root-Aggregate Model
//!
//! This processor works with the root-aggregate embedding model where:
//! - Only root nodes (no parent) of embeddable types get embedded
//! - Embeddings are stored in the `embedding` table, not on nodes
//! - The `stale` flag tracks which embeddings need regeneration
//! - The `modified_at` field tracks when embedding was marked stale (for debounce)

use crate::services::error::NodeServiceError;
use crate::services::NodeEmbeddingService;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;

/// Handle to wake the embedding processor
///
/// This is a lightweight, cloneable handle that can be passed to other services
/// (like `NodeService`) to trigger embedding processing when nodes change.
///
/// Multiple wakes are coalesced - the processor will process all stale embeddings
/// in a single run regardless of how many wake signals were sent.
#[derive(Clone)]
pub struct EmbeddingWaker {
    trigger_tx: mpsc::Sender<()>,
}

impl EmbeddingWaker {
    /// Wake the embedding processor to start processing
    ///
    /// Non-blocking. If the processor is already awake or has pending work,
    /// this is a no-op (signals are coalesced).
    pub fn wake(&self) {
        // Use try_send to avoid blocking - if channel is full, processor is already awake
        match self.trigger_tx.try_send(()) {
            Ok(_) => {
                tracing::debug!("EmbeddingProcessor wake signal sent");
            }
            Err(mpsc::error::TrySendError::Full(_)) => {
                // Channel full means processor will wake up anyway
                tracing::debug!("EmbeddingProcessor already has pending wake");
            }
            Err(mpsc::error::TrySendError::Closed(_)) => {
                tracing::warn!("EmbeddingProcessor has shut down, wake ignored");
            }
        }
    }
}

/// Embedding processor for background tasks
///
/// Processes stale embeddings in the background using the root-aggregate model.
/// Event-driven: sleeps until triggered, then processes until queue is empty.
///
/// Generic over connection type `C` to support both local and HTTP SurrealDB connections.
pub struct EmbeddingProcessor<C = surrealdb::engine::local::Db>
where
    C: surrealdb::Connection + 'static,
{
    waker: EmbeddingWaker,
    _shutdown_tx: mpsc::Sender<()>,
    _phantom: std::marker::PhantomData<C>,
}

impl<C> EmbeddingProcessor<C>
where
    C: surrealdb::Connection + 'static,
{
    /// Create and start embedding processor with background task
    ///
    /// Spawns an event-driven background task that:
    /// 1. Sleeps until triggered via `wake()` or `trigger_batch_embed()`
    /// 2. Processes ALL stale embeddings that have passed their debounce window
    /// 3. If pending embeddings exist, schedules a delayed wake for when debounce expires
    /// 4. Returns to sleep waiting for next trigger
    ///
    /// ## Event-Driven Model
    ///
    /// Unlike polling, this approach has zero overhead when idle. The processor
    /// only runs when there's actual work to do.
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
    pub fn new(embedding_service: Arc<NodeEmbeddingService<C>>) -> Result<Self, NodeServiceError> {
        tracing::info!("EmbeddingProcessor initializing (purely event-driven model)");

        let (trigger_tx, mut trigger_rx) = mpsc::channel::<()>(10);
        let (shutdown_tx, mut shutdown_rx) = mpsc::channel::<()>(1);

        // Get debounce duration from config (default 30s)
        let debounce_secs = embedding_service.config().debounce_duration_secs;
        let debounce_duration = Duration::from_secs(debounce_secs);

        // Spawn purely event-driven background task
        let service_clone = embedding_service.clone();
        let trigger_tx_clone = trigger_tx.clone();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    biased; // Check shutdown first

                    _ = shutdown_rx.recv() => {
                        tracing::info!("EmbeddingProcessor shutting down");
                        break;
                    }

                    Some(_) = trigger_rx.recv() => {
                        tracing::debug!("EmbeddingProcessor woken up by trigger");
                        // Drain any additional pending triggers (coalesce rapid triggers)
                        while trigger_rx.try_recv().is_ok() {}

                        // Process embeddings that have passed their debounce window
                        let has_pending = Self::process_until_empty(&service_clone).await;

                        // If there are pending embeddings that haven't passed debounce yet,
                        // schedule a delayed wake to process them later
                        if has_pending {
                            let tx = trigger_tx_clone.clone();
                            let delay = debounce_duration;
                            tokio::spawn(async move {
                                tokio::time::sleep(delay).await;
                                let _ = tx.try_send(());
                            });
                            tracing::debug!(
                                "Scheduled delayed wake in {}s for pending embeddings",
                                debounce_secs
                            );
                        }
                    }
                }
            }
        });

        let waker = EmbeddingWaker { trigger_tx };

        Ok(Self {
            waker,
            _shutdown_tx: shutdown_tx,
            _phantom: std::marker::PhantomData,
        })
    }

    /// Get a cloneable waker handle
    ///
    /// Use this to pass to other services (like `NodeService`) so they can
    /// wake the processor when embedding work is queued.
    pub fn waker(&self) -> EmbeddingWaker {
        self.waker.clone()
    }

    /// Process all stale embeddings until none remain
    ///
    /// Keeps processing batches until `process_stale_embeddings` returns 0.
    /// This ensures the queue is fully drained before returning to sleep.
    /// Yields between batches to prevent starving other async tasks.
    ///
    /// Returns true if there are pending stale embeddings that haven't passed
    /// their debounce window yet (requiring a delayed wake to be scheduled).
    async fn process_until_empty(service: &Arc<NodeEmbeddingService<C>>) -> bool {
        const BATCH_SIZE: usize = 10;
        let mut total_processed = 0;

        loop {
            match service.process_stale_embeddings(Some(BATCH_SIZE)).await {
                Ok(0) => {
                    // No more stale embeddings ready to process
                    if total_processed > 0 {
                        tracing::info!(
                            "EmbeddingProcessor finished - processed {} total embeddings",
                            total_processed
                        );
                    }
                    // Check if there are pending embeddings that haven't passed debounce yet
                    match service.has_pending_stale_embeddings().await {
                        Ok(has_pending) => {
                            if has_pending {
                                tracing::debug!(
                                    "Pending stale embeddings exist, will schedule delayed wake"
                                );
                            }
                            return has_pending;
                        }
                        Err(e) => {
                            tracing::warn!("Failed to check pending embeddings: {}", e);
                            return false;
                        }
                    }
                }
                Ok(count) => {
                    total_processed += count;
                    tracing::debug!(
                        "Processed {} embeddings (total: {})",
                        count,
                        total_processed
                    );
                    // Yield to allow other async tasks to run (backpressure)
                    tokio::task::yield_now().await;
                }
                Err(e) => {
                    tracing::error!(
                        "Embedding processing failed after {} embeddings: {}",
                        total_processed,
                        e
                    );
                    // Stop processing on error - will retry on next wake
                    return false;
                }
            }
        }
    }

    /// Wake the processor to start processing stale embeddings
    ///
    /// This is the primary way to trigger embedding processing. Call this
    /// after creating stale markers (e.g., after node create/update/delete).
    ///
    /// The wake signal is coalesced - multiple rapid wakes result in a single
    /// processing run that drains all stale embeddings.
    pub fn wake(&self) {
        self.waker.wake();
    }

    /// Trigger batch embedding immediately (alias for wake)
    ///
    /// Useful for explicit user actions like "Sync All" button or app startup.
    #[inline]
    pub fn trigger_batch_embed(&self) -> Result<(), NodeServiceError> {
        self.waker.wake();
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

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::mpsc;

    /// Test that EmbeddingWaker sends a signal when woken
    #[test]
    fn test_waker_wake_sends_signal() {
        let (trigger_tx, mut trigger_rx) = mpsc::channel::<()>(10);
        let waker = EmbeddingWaker { trigger_tx };

        // Wake should send signal
        waker.wake();

        // Verify signal was sent (non-blocking check)
        assert!(
            trigger_rx.try_recv().is_ok(),
            "Wake should have sent a signal"
        );
    }

    /// Test that multiple rapid wakes are coalesced (channel capacity behavior)
    #[test]
    fn test_waker_coalesces_multiple_wakes() {
        let (trigger_tx, mut trigger_rx) = mpsc::channel::<()>(2); // Small capacity
        let waker = EmbeddingWaker { trigger_tx };

        // Send multiple wakes rapidly
        waker.wake();
        waker.wake();
        waker.wake(); // Should be coalesced (channel full)

        // Drain and count - should be at most 2 (channel capacity)
        let mut count = 0;
        while trigger_rx.try_recv().is_ok() {
            count += 1;
        }

        assert!(
            count <= 2,
            "Excess wakes should be coalesced, got {} signals",
            count
        );
    }

    /// Test that waker handles closed channel gracefully
    #[test]
    fn test_waker_handles_closed_channel() {
        let (trigger_tx, trigger_rx) = mpsc::channel::<()>(10);
        let waker = EmbeddingWaker { trigger_tx };

        // Close the receiver
        drop(trigger_rx);

        // Wake should not panic, just log warning
        waker.wake(); // Should complete without panic
    }

    /// Test that waker is cloneable and all clones work
    #[test]
    fn test_waker_is_cloneable() {
        let (trigger_tx, mut trigger_rx) = mpsc::channel::<()>(10);
        let waker1 = EmbeddingWaker { trigger_tx };
        let waker2 = waker1.clone();

        waker1.wake();
        waker2.wake();

        // Both wakes should have sent signals
        assert!(trigger_rx.try_recv().is_ok(), "First wake should send");
        assert!(trigger_rx.try_recv().is_ok(), "Second wake should send");
    }
}
