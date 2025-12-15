use anyhow::Result;
use nodespace_core::db::DomainEvent;
use nodespace_core::NodeService;
use serde::Serialize;
use std::sync::Arc;
use tauri::{AppHandle, Emitter};
use tokio::sync::broadcast;
use tracing::{debug, error, info};

/// Payload for ID-only node/edge events (Issue #724)
///
/// Used for all node events (created, updated, deleted) to minimize payload size.
/// Frontend fetches full node data via get_node() API if needed.
#[derive(Serialize)]
struct NodeIdPayload {
    id: String,
}

/// Service for forwarding domain events to the Tauri frontend
///
/// Subscribes to domain events from NodeService and forwards them to the Tauri frontend,
/// filtering out events that originated from this Tauri client (prevents feedback loops).
/// Uses event-driven architecture with broadcast channels for efficient multi-subscriber support.
/// This is a bridge between the business logic layer (NodeService) and the frontend UI.
pub struct DomainEventForwarder {
    node_service: Arc<NodeService>,
    app: AppHandle,
    /// Client identifier for this Tauri window/instance
    /// Used to filter out events where source_client_id matches (prevent feedback loop)
    client_id: String,
}

impl DomainEventForwarder {
    /// Create a new DomainEventForwarder
    ///
    /// # Arguments
    ///
    /// * `node_service` - NodeService instance to subscribe to events from
    /// * `app` - Tauri AppHandle for emitting events to frontend
    /// * `client_id` - Unique identifier for this Tauri client (e.g., "tauri-main")
    pub fn new(node_service: Arc<NodeService>, app: AppHandle, client_id: String) -> Self {
        Self {
            node_service,
            app,
            client_id,
        }
    }

    /// Start the domain event forwarding service
    ///
    /// Subscribes to domain events from NodeService and forwards them
    /// to the Tauri frontend with the appropriate event names:
    /// - node:created, node:updated, node:deleted
    /// - edge:created, edge:updated, edge:deleted
    ///
    /// Filters out events where source_client_id matches this client's ID
    /// to prevent feedback loops from the Tauri frontend receiving its own changes.
    pub async fn run(self) -> Result<()> {
        info!(
            "🔧 Starting domain event forwarding service (client_id: {})",
            self.client_id
        );

        // Emit initial status
        self.emit_status("connected", None);

        // Subscribe to domain events from NodeService
        let mut rx = self.node_service.subscribe_to_events();

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
    /// Converts domain events from NodeService to Tauri events with proper naming and payload.
    /// Filters out events that originated from this client (prevents feedback loop).
    fn forward_event(&self, event: &DomainEvent) {
        // Extract source_client_id from the event
        let source_client_id = match event {
            DomainEvent::NodeCreated {
                source_client_id, ..
            } => source_client_id,
            DomainEvent::NodeUpdated {
                source_client_id, ..
            } => source_client_id,
            DomainEvent::NodeDeleted {
                source_client_id, ..
            } => source_client_id,
            DomainEvent::EdgeCreated {
                source_client_id, ..
            } => source_client_id,
            DomainEvent::EdgeUpdated {
                source_client_id, ..
            } => source_client_id,
            DomainEvent::EdgeDeleted {
                source_client_id, ..
            } => source_client_id,
            DomainEvent::CollectionMemberAdded {
                source_client_id, ..
            } => source_client_id,
            DomainEvent::CollectionMemberRemoved {
                source_client_id, ..
            } => source_client_id,
        };

        // Filter out events from this client (prevent feedback loop)
        if let Some(event_client_id) = source_client_id {
            if event_client_id == &self.client_id {
                debug!(
                    "Filtering out event from same client ({}): {:?}",
                    self.client_id, event
                );
                return;
            }
        }

        // Forward the event to the frontend (Issue #724: ID-only payloads)
        match event {
            DomainEvent::NodeCreated { node_id, .. } => {
                // Send only node_id - frontend fetches full data if needed
                let payload = NodeIdPayload {
                    id: node_id.clone(),
                };
                if let Err(e) = self.app.emit("node:created", &payload) {
                    error!("Failed to emit node:created for {}: {}", node_id, e);
                }
            }
            DomainEvent::NodeUpdated { node_id, .. } => {
                // Send only node_id - frontend fetches full data if needed
                let payload = NodeIdPayload {
                    id: node_id.clone(),
                };
                if let Err(e) = self.app.emit("node:updated", &payload) {
                    error!("Failed to emit node:updated for {}: {}", node_id, e);
                }
            }
            DomainEvent::NodeDeleted { id, .. } => {
                let payload = NodeIdPayload { id: id.clone() };
                if let Err(e) = self.app.emit("node:deleted", &payload) {
                    error!("Failed to emit node:deleted: {}", e);
                }
            }
            DomainEvent::EdgeCreated { relationship, .. } => {
                if let Err(e) = self.app.emit("edge:created", relationship) {
                    error!("Failed to emit edge:created: {}", e);
                }
            }
            DomainEvent::EdgeUpdated { relationship, .. } => {
                if let Err(e) = self.app.emit("edge:updated", relationship) {
                    error!("Failed to emit edge:updated: {}", e);
                }
            }
            DomainEvent::EdgeDeleted { id, .. } => {
                let payload = NodeIdPayload { id: id.clone() };
                if let Err(e) = self.app.emit("edge:deleted", &payload) {
                    error!("Failed to emit edge:deleted: {}", e);
                }
            }
            DomainEvent::CollectionMemberAdded { membership, .. } => {
                if let Err(e) = self.app.emit("collection:member-added", membership) {
                    error!("Failed to emit collection:member-added: {}", e);
                }
            }
            DomainEvent::CollectionMemberRemoved { membership, .. } => {
                if let Err(e) = self.app.emit("collection:member-removed", membership) {
                    error!("Failed to emit collection:member-removed: {}", e);
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
