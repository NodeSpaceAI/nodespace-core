use anyhow::Result;
use nodespace_core::db::DomainEvent;
use serde::Serialize;
use std::sync::Arc;
use tauri::{AppHandle, Emitter};
use tokio::sync::broadcast;
use tracing::{debug, error, info};

use nodespace_core::SurrealStore;

/// Payload for deleted node/edge events
#[derive(Serialize)]
struct DeletedPayload {
    id: String,
}

/// Service for managing real-time database synchronization
///
/// Subscribes to domain events from SurrealStore and forwards them to the Tauri frontend.
/// Uses event-driven architecture with broadcast channels for efficient multi-subscriber support.
pub struct LiveQueryService {
    store: Arc<SurrealStore>,
    app: AppHandle,
}

impl LiveQueryService {
    pub fn new(store: Arc<SurrealStore>, app: AppHandle) -> Self {
        Self { store, app }
    }

    /// Start the real-time synchronization service with event forwarding
    ///
    /// This implementation subscribes to domain events from SurrealStore and forwards them
    /// to the Tauri frontend with the appropriate event names:
    /// - node:created, node:updated, node:deleted
    /// - edge:created, edge:updated, edge:deleted
    pub async fn run(self) -> Result<()> {
        info!("🔧 Starting event-driven real-time synchronization service");

        // Emit initial status
        self.emit_status("connected", None);

        // Subscribe to domain events from SurrealStore
        let mut rx = self.store.subscribe_to_events();

        info!("✅ Event subscription established successfully");

        // Listen for events and forward to frontend
        loop {
            match rx.recv().await {
                Ok(event) => {
                    debug!("Received domain event: {:?}", event);
                    self.forward_event(&event);
                }
                Err(broadcast::error::RecvError::Lagged(_)) => {
                    // Event queue was too long, skip missed events
                    debug!("Event queue lagged, some events may have been skipped");
                }
                Err(broadcast::error::RecvError::Closed) => {
                    // Broadcast channel closed, service should shut down
                    error!("Event broadcast channel closed, stopping synchronization service");
                    self.emit_status("disconnected", Some("channel-closed"));
                    return Err(anyhow::anyhow!("Event broadcast channel closed"));
                }
            }
        }
    }

    /// Forward domain event to Tauri frontend
    ///
    /// Converts domain events from SurrealStore to Tauri events with proper naming and payload.
    fn forward_event(&self, event: &DomainEvent) {
        match event {
            DomainEvent::NodeCreated(node) => {
                if let Err(e) = self.app.emit("node:created", node) {
                    error!("Failed to emit node:created: {}", e);
                }
            }
            DomainEvent::NodeUpdated(node) => {
                if let Err(e) = self.app.emit("node:updated", node) {
                    error!("Failed to emit node:updated: {}", e);
                }
            }
            DomainEvent::NodeDeleted { id } => {
                let payload = DeletedPayload { id: id.clone() };
                if let Err(e) = self.app.emit("node:deleted", &payload) {
                    error!("Failed to emit node:deleted: {}", e);
                }
            }
            DomainEvent::EdgeCreated(relationship) => {
                if let Err(e) = self.app.emit("edge:created", relationship) {
                    error!("Failed to emit edge:created: {}", e);
                }
            }
            DomainEvent::EdgeUpdated(relationship) => {
                if let Err(e) = self.app.emit("edge:updated", relationship) {
                    error!("Failed to emit edge:updated: {}", e);
                }
            }
            DomainEvent::EdgeDeleted { id } => {
                let payload = DeletedPayload { id: id.clone() };
                if let Err(e) = self.app.emit("edge:deleted", &payload) {
                    error!("Failed to emit edge:deleted: {}", e);
                }
            }
        }
    }

    /// Emit synchronization status event
    fn emit_status(&self, status: &str, reason: Option<&str>) {
        #[derive(Serialize)]
        struct StatusPayload {
            status: String,
            reason: Option<String>,
        }

        let payload = StatusPayload {
            status: status.to_string(),
            reason: reason.map(|s| s.to_string()),
        };

        if let Err(e) = self.app.emit("sync:status", &payload) {
            error!("Failed to emit sync:status: {}", e);
        }
    }
}
