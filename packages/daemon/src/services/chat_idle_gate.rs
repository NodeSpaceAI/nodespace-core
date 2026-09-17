//! Process-global idle gate for the chat/completion model.
//!
//! Background work that wants the chat model (today: ai-chat title
//! generation) must not compete with a live conversation turn. This gate is
//! how it waits its turn.
//!
//! # Why a gate above the engine rather than the engine's own lock
//!
//! `ChatEngine` already serialises generations with an `inference_lock`
//! (`packages/nlp-engine/src/chat/mod.rs`), but that is a plain `tokio::Mutex`
//! and therefore *unfair*: a background job that reaches it first holds it for
//! the full generation, and the user's live turn queues behind a job nobody
//! asked for. Mutual exclusion is not priority. This gate sits above
//! `generate()` and encodes the priority the mutex cannot.
//!
//! # Why process-global rather than per-database
//!
//! `LocalAgentServiceImpl::has_active_turns()` already answers "is a turn in
//! flight?", but `turn_tokens` lives on the per-database
//! `LocalAgentServiceInner` while the engine is shared process-wide via
//! `SharedLocalAgent` (ADR-053 scopes *compute* per database; the model
//! weights are still one instance). A per-database gate would happily run
//! database A's title job straight into database B's live turn. So the
//! counter lives here, on the shared handle, and every database's turns
//! register against it.
//!
//! # Shape
//!
//! Deliberately mirrors `EmbeddingScheduler`
//! (`packages/core/src/services/embedding_processor.rs`), which solves the
//! same active-first problem for the embedding model. Two details are copied
//! exactly because both are easy to get subtly wrong:
//!
//! - a turn registers itself *before* contending for anything, so a job
//!   checking the counter cannot slip between "turn decided to run" and "turn
//!   became visible"; and
//! - a waiter arms its `Notified` future *before* re-reading the counter, so a
//!   turn ending in that window still wakes it (no lost wakeup).
//!
//! Unlike the embedding scheduler there is no semaphore here. This gate only
//! answers "may background work run now?" — the engine's own lock still
//! provides mutual exclusion, and background jobs are serialised by the single
//! worker that runs them.

use std::sync::atomic::{AtomicUsize, Ordering};

use tokio::sync::Notify;

/// Tracks live chat turns so background work can wait for the model to go idle.
#[derive(Debug, Default)]
pub struct ChatIdleGate {
    /// Live conversation turns, across every open database.
    active_turns: AtomicUsize,
    /// Wakes background waiters when `active_turns` reaches zero.
    idle: Notify,
}

/// RAII marker that a live turn is in flight.
///
/// Registration is tied to the guard's lifetime rather than to paired
/// `begin`/`end` calls so that an early return, a `?`, a panic, or a cancelled
/// turn future cannot leave the counter stuck above zero — which would wedge
/// background titling permanently, and silently.
#[derive(Debug)]
pub struct ActiveTurnGuard<'a> {
    gate: &'a ChatIdleGate,
}

impl Drop for ActiveTurnGuard<'_> {
    fn drop(&mut self) {
        // `fetch_sub` returns the *previous* value: 1 means this was the last
        // turn and the model is now idle.
        if self.gate.active_turns.fetch_sub(1, Ordering::SeqCst) == 1 {
            self.gate.idle.notify_waiters();
        }
    }
}

impl ChatIdleGate {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a live turn. The model counts as busy until the returned guard
    /// is dropped.
    pub fn begin_turn(&self) -> ActiveTurnGuard<'_> {
        self.active_turns.fetch_add(1, Ordering::SeqCst);
        ActiveTurnGuard { gate: self }
    }

    /// Whether any live turn is currently in flight.
    pub fn is_busy(&self) -> bool {
        self.active_turns.load(Ordering::SeqCst) > 0
    }

    /// Wait until no live turn is in flight.
    ///
    /// Returns immediately when already idle. This is a point-in-time check,
    /// not a reservation: a turn may start the moment this returns, so callers
    /// re-check rather than assuming exclusivity (see `wait_for_idle_stable`).
    pub async fn wait_for_idle(&self) {
        loop {
            // Arm before re-reading, so a turn that ends between the read and
            // the await still wakes us.
            let notified = self.idle.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();

            if !self.is_busy() {
                return;
            }
            notified.await;
        }
    }

    /// Wait for the model to be idle and to *stay* idle for `quiet_for`.
    ///
    /// A conversation is a burst of turns with short gaps between them. Firing
    /// background work into the first of those gaps would technically satisfy
    /// "only when idle" while still landing in the middle of the user's
    /// conversation. Requiring a quiet period means background work runs when
    /// the user has actually stopped, not merely between two of their
    /// messages.
    pub async fn wait_for_idle_stable(&self, quiet_for: std::time::Duration) {
        loop {
            self.wait_for_idle().await;
            tokio::time::sleep(quiet_for).await;
            if !self.is_busy() {
                return;
            }
            // A turn started during the quiet period — wait for the next lull.
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::time::Duration;

    #[tokio::test]
    async fn idle_when_no_turns() {
        let gate = ChatIdleGate::new();
        assert!(!gate.is_busy());
        // Returns immediately rather than hanging.
        gate.wait_for_idle().await;
    }

    #[tokio::test]
    async fn busy_while_a_turn_is_held() {
        let gate = ChatIdleGate::new();
        let guard = gate.begin_turn();
        assert!(gate.is_busy());
        drop(guard);
        assert!(!gate.is_busy());
    }

    #[tokio::test]
    async fn concurrent_turns_keep_it_busy_until_the_last_one_ends() {
        let gate = ChatIdleGate::new();
        let a = gate.begin_turn();
        let b = gate.begin_turn();
        assert!(gate.is_busy());
        drop(a);
        assert!(gate.is_busy(), "still busy while the second turn runs");
        drop(b);
        assert!(!gate.is_busy());
    }

    #[tokio::test]
    async fn waiter_wakes_when_the_turn_ends() {
        let gate = Arc::new(ChatIdleGate::new());
        let guard = gate.begin_turn();

        let waiter_gate = gate.clone();
        let waiter = tokio::spawn(async move {
            waiter_gate.wait_for_idle().await;
        });

        // The waiter must still be parked while the turn is held.
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(!waiter.is_finished(), "waiter woke while a turn was active");

        drop(guard);
        tokio::time::timeout(Duration::from_secs(5), waiter)
            .await
            .expect("waiter did not wake after the turn ended")
            .expect("waiter task panicked");
    }

    #[tokio::test]
    async fn guard_releases_on_panic() {
        let gate = Arc::new(ChatIdleGate::new());
        let panicking = gate.clone();
        let handle = tokio::spawn(async move {
            let _guard = panicking.begin_turn();
            panic!("turn blew up");
        });
        assert!(handle.await.is_err(), "task was expected to panic");
        assert!(
            !gate.is_busy(),
            "a panicking turn must not wedge the gate busy forever"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn stable_wait_requires_a_quiet_period() {
        let gate = Arc::new(ChatIdleGate::new());
        let quiet = Duration::from_secs(10);

        let waiter_gate = gate.clone();
        let waiter = tokio::spawn(async move {
            waiter_gate.wait_for_idle_stable(quiet).await;
        });

        // A turn starts partway through the quiet period, which must restart it.
        tokio::time::sleep(Duration::from_secs(5)).await;
        let guard = gate.begin_turn();
        tokio::time::sleep(Duration::from_secs(10)).await;
        assert!(
            !waiter.is_finished(),
            "quiet period must restart when a turn interrupts it"
        );

        drop(guard);
        tokio::time::timeout(Duration::from_secs(60), waiter)
            .await
            .expect("waiter did not settle after the model went quiet")
            .expect("waiter task panicked");
    }
}
