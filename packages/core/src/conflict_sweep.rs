//! Conflict-journal reconciliation sweep (ADR-068, conflict-journal-and-
//! resolution.md §5.4): a periodic background pass that closes open
//! conflict records whose participants are gone or no longer collide.
//!
//! Not required for S1-S3 correctness — a stale open record for a departed
//! node is a stale row, not a wrong one — but required before the Conflicts
//! view is fully trustworthy, and the backstop for §4's accepted TOCTOU gap
//! in the pre-write-check/post-write-mark detection pattern.
//!
//! Modeled structurally on `playbook::cron_runner::cron_runner_loop`'s
//! shutdown-aware `tokio::select!` shape and its `watch::Receiver<bool>`
//! shutdown signal — the same in-crate idiom, not a new dependency.

use crate::services::NodeService;
use std::sync::Arc;
use std::time::Duration;
use tracing::{debug, warn};

/// Interval between reconciliation sweeps. A backstop pass, not the primary
/// detection path (inline detection already runs on every create/update) —
/// low frequency is intentional.
const SWEEP_INTERVAL: Duration = Duration::from_secs(15 * 60);

/// Run the reconciliation sweep loop for one database's `NodeService`.
/// Exits when `shutdown_rx` receives `true` or the watch sender is dropped.
pub async fn conflict_sweep_loop(
    node_service: Arc<NodeService>,
    mut shutdown_rx: tokio::sync::watch::Receiver<bool>,
) {
    debug!(
        "Conflict reconciliation sweep started, running every {} seconds",
        SWEEP_INTERVAL.as_secs()
    );

    loop {
        tokio::select! {
            () = tokio::time::sleep(SWEEP_INTERVAL) => {
                match node_service.reconcile_conflicts().await {
                    Ok(closed) if closed > 0 => {
                        debug!(closed, "Conflict reconciliation sweep closed stale records");
                    }
                    Ok(_) => {}
                    Err(e) => {
                        warn!(error = %e, "Conflict reconciliation sweep failed");
                    }
                }
            }
            _ = shutdown_rx.changed() => {
                if *shutdown_rx.borrow() {
                    debug!("Conflict reconciliation sweep received shutdown signal");
                    break;
                }
            }
        }
    }

    debug!("Conflict reconciliation sweep stopped");
}
