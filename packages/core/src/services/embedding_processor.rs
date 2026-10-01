//! Background Embedding Processor
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
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::{mpsc, Notify, Semaphore, SemaphorePermit};

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

/// Process-global gate that gives the active database's embedding work priority
/// over every other open database (ADR-053: per-database compute scoping).
///
/// The embedding model is a single process-global resource, so only one database
/// may run a batch through it at a time. This scheduler enforces that mutual
/// exclusion *and* a priority: while the active database has embedding batches
/// queued, batches from other databases defer. When the active database drains,
/// the others proceed. Priority is re-evaluated per batch, so a non-active
/// database is never starved permanently — it resumes the moment the active
/// database goes idle.
///
/// The scheduler keys on the database id as a plain [`String`] (the registry
/// ULID) so this core type stays free of any daemon-side identifier type.
pub struct EmbeddingScheduler {
    /// The database whose live edit stream marks it active, if any.
    active: Mutex<Option<String>>,
    /// Single-permit gate: only one database runs a batch through the shared
    /// model at a time.
    gate: Semaphore,
    /// Count of active-database batches queued or in flight. Non-active batches
    /// wait while this is non-zero.
    high_waiters: AtomicUsize,
    /// Wakes deferred non-active waiters when the active backlog drains.
    notify: Notify,
}

/// A held scheduler slot. Dropping it releases the shared-model gate and, for an
/// active-database batch, drops the [`HighPriorityReservation`] which decrements
/// the active-waiter count and wakes any deferred non-active waiters so priority
/// is re-evaluated.
pub struct SchedulerPermit<'a> {
    _permit: SemaphorePermit<'a>,
    /// Present only for an active-database batch.
    _reservation: Option<HighPriorityReservation<'a>>,
}

/// RAII active-waiter reservation. Bumping `high_waiters` *before* the gate is
/// acquired is what makes concurrent non-active batches defer; guarding it in a
/// drop type ensures an `acquire` future cancelled while awaiting the gate can
/// never leak the count (a leak would defer every non-active database forever).
struct HighPriorityReservation<'a> {
    scheduler: &'a EmbeddingScheduler,
}

impl<'a> HighPriorityReservation<'a> {
    fn new(scheduler: &'a EmbeddingScheduler) -> Self {
        scheduler.high_waiters.fetch_add(1, Ordering::SeqCst);
        Self { scheduler }
    }
}

impl Drop for HighPriorityReservation<'_> {
    fn drop(&mut self) {
        self.scheduler.high_waiters.fetch_sub(1, Ordering::SeqCst);
        // Wake deferred non-active waiters so they re-check the backlog.
        self.scheduler.notify.notify_waiters();
    }
}

impl Default for EmbeddingScheduler {
    fn default() -> Self {
        Self::new()
    }
}

impl EmbeddingScheduler {
    /// Create a scheduler with no active database.
    pub fn new() -> Self {
        Self {
            active: Mutex::new(None),
            gate: Semaphore::new(1),
            high_waiters: AtomicUsize::new(0),
            notify: Notify::new(),
        }
    }

    /// Mark which database is active (the one with a live edit stream), or clear
    /// it with `None`. The active database's embedding batches take priority.
    pub fn set_active(&self, db_id: Option<String>) {
        *self.active.lock().expect("scheduler active lock poisoned") = db_id;
    }

    /// The currently active database id, if any.
    pub fn active_id(&self) -> Option<String> {
        self.active
            .lock()
            .expect("scheduler active lock poisoned")
            .clone()
    }

    /// Clear the active database id, but ONLY if it still names `db_id`.
    ///
    /// Used when a live `WatchNodes` stream ends (idle eviction, client
    /// disconnect, or a natural close/error) to release the active claim it
    /// took in `set_active` on open. A plain `set_active(None)` would be
    /// wrong here: `active` holds at most one id, so if a NEWER stream (on a
    /// different database) has already called `set_active` since this one
    /// opened, an unconditional clear from the older stream's teardown would
    /// clobber the newer stream's claim instead of just retracting its own —
    /// leaving the actually-active database evictable while it still has a
    /// live subscriber. Comparing before clearing makes teardown idempotent
    /// and order-independent: it only ever retracts a still-current claim.
    pub fn clear_active_if(&self, db_id: &str) {
        let mut active = self.active.lock().expect("scheduler active lock poisoned");
        if active.as_deref() == Some(db_id) {
            *active = None;
        }
    }

    /// Whether `db_id` is currently the active database.
    pub fn is_active(&self, db_id: &str) -> bool {
        self.active
            .lock()
            .expect("scheduler active lock poisoned")
            .as_deref()
            == Some(db_id)
    }

    /// Acquire the shared-model gate for one batch on behalf of `db_id`.
    ///
    /// The active database registers itself as a high-priority waiter and then
    /// contends for the gate directly. A non-active database first waits until no
    /// active-database batches are queued, then contends for the gate. The
    /// returned permit must be dropped between batches so priority is
    /// re-evaluated each time.
    pub async fn acquire(&self, db_id: &str) -> SchedulerPermit<'_> {
        if self.is_active(db_id) {
            // Register as a high-priority waiter BEFORE contending for the gate so
            // any concurrent non-active waiter sees this batch and defers. The
            // reservation is RAII-guarded, so if this future is dropped while
            // awaiting the gate the count is released rather than leaked.
            let reservation = HighPriorityReservation::new(self);
            let permit = self
                .gate
                .acquire()
                .await
                .expect("scheduler semaphore never closes");
            SchedulerPermit {
                _permit: permit,
                _reservation: Some(reservation),
            }
        } else {
            // Defer while the active database has batches queued. Register on the
            // notifier *before* re-checking the count so an active batch that
            // drains between the check and the await cannot be missed.
            loop {
                let notified = self.notify.notified();
                tokio::pin!(notified);
                notified.as_mut().enable();
                if self.high_waiters.load(Ordering::SeqCst) == 0 {
                    break;
                }
                notified.await;
            }
            let permit = self
                .gate
                .acquire()
                .await
                .expect("scheduler semaphore never closes");
            SchedulerPermit {
                _permit: permit,
                _reservation: None,
            }
        }
    }
}

/// Embedding processor for background tasks
///
/// Processes stale embeddings in the background using the root-aggregate model.
/// Event-driven: sleeps until triggered, then processes until queue is empty.
pub struct EmbeddingProcessor {
    waker: EmbeddingWaker,
    _shutdown_tx: mpsc::Sender<()>,
}

impl EmbeddingProcessor {
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
    /// ## Root-Aggregate Model
    ///
    /// Uses `process_stale_embeddings()` which:
    /// 1. Queries the `embedding` table for stale entries
    /// 2. For each stale root node, aggregates its subtree content
    /// 3. Generates new embeddings and updates the table
    ///
    /// # Arguments
    /// * `embedding_service` - The embedding service for processing nodes
    /// * `scheduler` - Process-global gate that grants the active database's
    ///   batches priority over other databases (ADR-053: per-database compute
    ///   scoping)
    /// * `db_id` - This database's registry id, used to ask the scheduler
    ///   whether this database is the active one
    ///
    /// # Returns
    /// A new EmbeddingProcessor instance with active background task
    pub fn new(
        embedding_service: Arc<NodeEmbeddingService>,
        scheduler: Arc<EmbeddingScheduler>,
        db_id: String,
    ) -> Result<Self, NodeServiceError> {
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
                        let has_pending =
                            Self::process_until_empty(&service_clone, &scheduler, &db_id).await;

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
    ///
    /// Each batch runs under a [`SchedulerPermit`] so the active database's
    /// embedding work takes priority over other open databases (ADR-053). The
    /// permit is released between batches, re-evaluating priority every batch.
    async fn process_until_empty(
        service: &Arc<NodeEmbeddingService>,
        scheduler: &Arc<EmbeddingScheduler>,
        db_id: &str,
    ) -> bool {
        const BATCH_SIZE: usize = 10;
        let mut total_processed = 0;

        loop {
            // Hold the shared-model gate only for the duration of one batch, then
            // drop it before checking for more work so a higher-priority database
            // can interleave.
            let batch_result = {
                let _permit = scheduler.acquire(db_id).await;
                service.process_stale_embeddings(Some(BATCH_SIZE)).await
            };
            match batch_result {
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

    #[test]
    fn scheduler_tracks_active_database() {
        let scheduler = EmbeddingScheduler::new();
        assert!(scheduler.active_id().is_none());
        assert!(!scheduler.is_active("A"));

        scheduler.set_active(Some("A".to_string()));
        assert!(scheduler.is_active("A"));
        assert!(!scheduler.is_active("B"));
        assert_eq!(scheduler.active_id().as_deref(), Some("A"));

        scheduler.set_active(None);
        assert!(!scheduler.is_active("A"));
        assert!(scheduler.active_id().is_none());
    }

    /// With no active database, a batch acquires the gate immediately (no
    /// deferral) — the single-database path is unaffected.
    #[tokio::test]
    async fn scheduler_grants_immediately_when_no_active_database() {
        let scheduler = EmbeddingScheduler::new();
        let permit = scheduler.acquire("only-db").await;
        drop(permit);
        // A second acquire also succeeds immediately (gate released on drop).
        let _permit = scheduler.acquire("only-db").await;
    }

    /// The active database's batch is granted the shared-model gate ahead of a
    /// non-active database's batch that was requested first — proving
    /// active-first priority without loading any embedding model.
    #[tokio::test]
    async fn scheduler_prioritizes_active_over_earlier_queued_non_active() {
        // Distinguishes active-first priority from a plain FIFO gate: a non-active
        // batch requests the gate BEFORE a second active batch, yet the active one
        // must still be granted first. A FIFO gate would grant the earlier request
        // and yield ["B", "A2"].
        let scheduler = Arc::new(EmbeddingScheduler::new());
        scheduler.set_active(Some("A".to_string()));

        let order: Arc<Mutex<Vec<&'static str>>> = Arc::new(Mutex::new(Vec::new()));

        // An active batch holds the gate (high_waiters -> 1).
        let a1 = scheduler.acquire("A").await;

        // A non-active batch requests next. Because active work is pending it
        // parks on the notifier instead of queueing on the gate. The single-thread
        // test runtime runs it to that park on one yield.
        let sched_b = scheduler.clone();
        let order_b = order.clone();
        let b_task = tokio::spawn(async move {
            let _p = sched_b.acquire("B").await;
            order_b.lock().unwrap().push("B");
        });
        tokio::task::yield_now().await;

        // A SECOND active batch requests *after* B and queues on the gate. Reading
        // the private counter is possible because this test lives in-module.
        let sched_a2 = scheduler.clone();
        let order_a2 = order.clone();
        let a2_task = tokio::spawn(async move {
            let _p = sched_a2.acquire("A").await;
            order_a2.lock().unwrap().push("A2");
        });
        while scheduler.high_waiters.load(Ordering::SeqCst) < 2 {
            tokio::task::yield_now().await;
        }

        // Release the first active batch. The second active batch wins the gate
        // over the earlier-requested non-active batch, which stays parked until
        // all active work drains.
        drop(a1);

        a2_task.await.unwrap();
        b_task.await.unwrap();

        assert_eq!(
            *order.lock().unwrap(),
            vec!["A2", "B"],
            "an active batch requested after a non-active one must still be granted first"
        );
    }
}
