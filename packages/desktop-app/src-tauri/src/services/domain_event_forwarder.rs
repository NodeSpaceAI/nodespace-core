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
#[serde(rename_all = "camelCase")]
struct NodeIdPayload {
    id: String,
    /// Optional node type - included for node:created to enable reactive UI updates
    /// (e.g., collections sidebar reacts to new collection nodes without fetching)
    #[serde(skip_serializing_if = "Option::is_none")]
    node_type: Option<String>,
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
    ///
    /// Issue #811: All relationship events use unified format (RelationshipCreated/Updated/Deleted).
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
            // Unified relationship events (Issue #811)
            DomainEvent::RelationshipCreated {
                source_client_id, ..
            } => source_client_id,
            DomainEvent::RelationshipUpdated {
                source_client_id, ..
            } => source_client_id,
            DomainEvent::RelationshipDeleted {
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
            DomainEvent::NodeCreated {
                node_id, node_type, ..
            } => {
                // Include node_type for reactive UI updates (e.g., collections sidebar)
                let payload = NodeIdPayload {
                    id: node_id.clone(),
                    node_type: Some(node_type.clone()),
                };
                if let Err(e) = self.app.emit("node:created", &payload) {
                    error!("Failed to emit node:created for {}: {}", node_id, e);
                }
            }
            DomainEvent::NodeUpdated { node_id, .. } => {
                // Send only node_id - frontend fetches full data if needed
                let payload = NodeIdPayload {
                    id: node_id.clone(),
                    node_type: None,
                };
                if let Err(e) = self.app.emit("node:updated", &payload) {
                    error!("Failed to emit node:updated for {}: {}", node_id, e);
                }
            }
            DomainEvent::NodeDeleted { id, .. } => {
                let payload = NodeIdPayload {
                    id: id.clone(),
                    node_type: None,
                };
                if let Err(e) = self.app.emit("node:deleted", &payload) {
                    error!("Failed to emit node:deleted: {}", e);
                }
            }
            // Unified relationship events (Issue #811)
            // All relationship types (has_child, member_of, mentions, custom) use these events
            DomainEvent::RelationshipCreated { relationship, .. } => {
                debug!(
                    "Forwarding RelationshipCreated: {} ({})",
                    relationship.id, relationship.relationship_type
                );
                if let Err(e) = self.app.emit("relationship:created", relationship) {
                    error!("Failed to emit relationship:created: {}", e);
                }
            }
            DomainEvent::RelationshipUpdated { relationship, .. } => {
                debug!(
                    "Forwarding RelationshipUpdated: {} ({})",
                    relationship.id, relationship.relationship_type
                );
                if let Err(e) = self.app.emit("relationship:updated", relationship) {
                    error!("Failed to emit relationship:updated: {}", e);
                }
            }
            DomainEvent::RelationshipDeleted {
                id,
                from_id,
                to_id,
                relationship_type,
                ..
            } => {
                #[derive(Serialize)]
                #[serde(rename_all = "camelCase")]
                struct RelationshipDeletedPayload {
                    id: String,
                    from_id: String,
                    to_id: String,
                    relationship_type: String,
                }
                let payload = RelationshipDeletedPayload {
                    id: id.clone(),
                    from_id: from_id.clone(),
                    to_id: to_id.clone(),
                    relationship_type: relationship_type.clone(),
                };
                debug!(
                    "Forwarding RelationshipDeleted: {} ({}) from {} to {}",
                    id, relationship_type, from_id, to_id
                );
                if let Err(e) = self.app.emit("relationship:deleted", &payload) {
                    error!("Failed to emit relationship:deleted: {}", e);
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
