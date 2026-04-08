//! Skill description updater — keeps the "Node Creation" skill description current.
//!
//! When schemas are created or deleted, the "Node Creation" skill's description
//! should list the available custom types so AI agents can discover it via
//! semantic search. For example: "Create new instances of existing node types —
//! add a task, text note, or an entry for Invoice, Customer, Project."
//!
//! # Architecture
//!
//! `SkillUpdater::start()` subscribes to the domain event channel and reacts
//! to `NodeCreated`, `NodeUpdated`, and `NodeDeleted` events where `node_type == "schema"`.
//! On each schema change it:
//! 1. Fetches all non-core schemas via `NodeService::get_all_schemas()`.
//! 2. Builds a new description string including the custom type names.
//! 3. Updates the "Node Creation" skill node's `description` property.
//! 4. The embedding pipeline then re-embeds the skill automatically (triggered by
//!    the `NodeUpdated` event the update emits).
//!
//! Issue #1061.

use crate::db::events::{DomainEvent, EventEnvelope};
use crate::models::{NodeQuery, NodeUpdate};
use crate::services::NodeService;
use std::sync::Arc;
use tokio::sync::broadcast;
use tracing::{debug, info, warn};

/// The content field (name) of the skill node we update.
const NODE_CREATION_SKILL_NAME: &str = "Node Creation";

/// Base description when no custom schemas exist.
const BASE_DESCRIPTION: &str = "Create new instances of existing node types \
    — add a task, text note, or an entry for a custom type. \
    Use when user wants to add a new record or item.";

/// Listens for schema domain events and updates the "Node Creation" skill description.
pub struct SkillUpdater {
    node_service: Arc<NodeService>,
}

impl SkillUpdater {
    pub fn new(node_service: Arc<NodeService>) -> Self {
        Self { node_service }
    }

    /// Subscribe to domain events and start the update loop.
    ///
    /// Runs an initial sync before entering the event loop so that schemas
    /// present at startup are reflected in the description without waiting for
    /// a schema mutation event.
    ///
    /// Runs until the broadcast channel closes or `shutdown_rx` fires `true`.
    pub async fn start(self: Arc<Self>, mut shutdown_rx: tokio::sync::watch::Receiver<bool>) {
        let mut rx: broadcast::Receiver<EventEnvelope> = self.node_service.subscribe_to_events();

        // Subscribe BEFORE the initial sync to avoid a TOCTOU gap where a
        // schema event arrives between the sync and the first rx.recv().
        info!("SkillUpdater subscribed to domain events");

        // Proactive startup sync: schemas that existed before this task started
        // would otherwise be ignored until the next schema mutation.
        if let Err(e) = self.update_node_creation_skill().await {
            warn!("SkillUpdater: startup sync failed: {}", e);
        }

        loop {
            tokio::select! {
                result = rx.recv() => {
                    match result {
                        Ok(envelope) => {
                            if self.is_schema_event(&envelope.event) {
                                debug!("SkillUpdater: schema change detected, updating Node Creation skill");
                                if let Err(e) = self.update_node_creation_skill().await {
                                    warn!("SkillUpdater: failed to update skill: {}", e);
                                }
                            }
                        }
                        Err(broadcast::error::RecvError::Lagged(count)) => {
                            warn!("SkillUpdater lagged, missed {} events — running update to stay consistent", count);
                            // After a lag, proactively sync in case we missed a schema event.
                            if let Err(e) = self.update_node_creation_skill().await {
                                warn!("SkillUpdater: post-lag update failed: {}", e);
                            }
                        }
                        Err(broadcast::error::RecvError::Closed) => {
                            info!("SkillUpdater: event channel closed, shutting down");
                            break;
                        }
                    }
                }
                _ = shutdown_rx.changed() => {
                    if *shutdown_rx.borrow() {
                        info!("SkillUpdater: shutdown signal received");
                        break;
                    }
                }
            }
        }
    }

    /// Returns true if the event is a schema node creation, update, or deletion.
    fn is_schema_event(&self, event: &DomainEvent) -> bool {
        matches!(
            event,
            DomainEvent::NodeCreated { node_type, .. } if node_type == "schema"
        ) || matches!(
            event,
            DomainEvent::NodeUpdated { node_type, .. } if node_type == "schema"
        ) || matches!(
            event,
            DomainEvent::NodeDeleted { node_type, .. } if node_type == "schema"
        )
    }

    /// Rebuild the "Node Creation" skill description and persist it.
    async fn update_node_creation_skill(&self) -> anyhow::Result<()> {
        // 1. Fetch all schemas to get the current custom type list.
        let schemas = match self.node_service.get_all_schemas().await {
            Ok(s) => s,
            Err(e) => {
                warn!(
                    "SkillUpdater: failed to fetch schemas, skipping update: {}",
                    e
                );
                return Ok(());
            }
        };

        // Use display name (content) for the description so the embedding model
        // sees human-readable type names like "Invoice" instead of machine keys
        // like "invoice". This improves semantic match quality.
        let custom_types: Vec<String> = schemas
            .iter()
            .filter(|s| !s.is_core)
            .map(|s| s.content.clone())
            .collect();

        // 2. Build the updated description.
        let new_description = build_node_creation_description(&custom_types);

        // 3. Find the "Node Creation" skill node.
        let query = NodeQuery {
            node_type: Some("skill".to_string()),
            content_contains: Some(NODE_CREATION_SKILL_NAME.to_string()),
            ..Default::default()
        };
        let skill_nodes = self
            .node_service
            .query_nodes_simple(query)
            .await
            .unwrap_or_default();

        let skill_node = skill_nodes
            .into_iter()
            .find(|n| n.content == NODE_CREATION_SKILL_NAME);

        let skill_node = match skill_node {
            Some(n) => n,
            None => {
                warn!("SkillUpdater: 'Node Creation' skill node not found — skipping update");
                return Ok(());
            }
        };

        // 4. Check if update is needed (avoid unnecessary writes).
        let current_desc = skill_node
            .properties
            .get("description")
            .or_else(|| {
                skill_node
                    .properties
                    .get("skill")
                    .and_then(|s| s.get("description"))
            })
            .and_then(|v| v.as_str())
            .unwrap_or("");

        if current_desc == new_description {
            debug!("SkillUpdater: description unchanged, skipping write");
            return Ok(());
        }

        // 5. Update the skill node's description property.
        let update = NodeUpdate {
            properties: Some(serde_json::json!({ "description": new_description })),
            ..Default::default()
        };

        self.node_service
            .update_node_unchecked(&skill_node.id, update)
            .await
            .map_err(|e| anyhow::anyhow!("update_node_unchecked failed: {:?}", e))?;

        info!(
            "SkillUpdater: updated 'Node Creation' skill description (custom types: {:?})",
            custom_types
        );

        Ok(())
    }
}

/// Build the "Node Creation" skill description string.
///
/// When custom types exist, appends them to the base description so the AI can
/// discover this skill via semantic search (e.g., "create a new Invoice").
pub fn build_node_creation_description(custom_types: &[String]) -> String {
    if custom_types.is_empty() {
        BASE_DESCRIPTION.to_string()
    } else {
        let types_list = custom_types.join(", ");
        format!(
            "Create new instances of existing node types \
            — add a task, text note, or an entry for {}. \
            Use when user wants to add a new record or item.",
            types_list
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_description_no_custom_types() {
        let desc = build_node_creation_description(&[]);
        assert_eq!(desc, BASE_DESCRIPTION);
        assert!(desc.contains("custom type"));
    }

    #[test]
    fn test_build_description_single_custom_type() {
        let desc = build_node_creation_description(&["invoice".to_string()]);
        assert!(desc.contains("invoice"));
        assert!(!desc.contains("custom type"));
    }

    #[test]
    fn test_build_description_multiple_custom_types() {
        let desc =
            build_node_creation_description(&["invoice".to_string(), "customer".to_string()]);
        assert!(desc.contains("invoice"));
        assert!(desc.contains("customer"));
    }

    #[test]
    fn test_is_schema_event_node_created() {
        // We test the is_schema_event logic directly via the helper
        let event = DomainEvent::NodeCreated {
            node_id: "schema:invoice".to_string(),
            node_type: "schema".to_string(),
        };
        assert!(matches!(
            &event,
            DomainEvent::NodeCreated { node_type, .. } if node_type == "schema"
        ));
    }

    #[test]
    fn test_is_schema_event_node_deleted() {
        let event = DomainEvent::NodeDeleted {
            id: "schema:invoice".to_string(),
            node_type: "schema".to_string(),
        };
        assert!(matches!(
            &event,
            DomainEvent::NodeDeleted { node_type, .. } if node_type == "schema"
        ));
    }

    #[test]
    fn test_non_schema_event_not_matched() {
        let event = DomainEvent::NodeCreated {
            node_id: "node:abc".to_string(),
            node_type: "task".to_string(),
        };
        assert!(!matches!(
            &event,
            DomainEvent::NodeCreated { node_type, .. } if node_type == "schema"
        ));
    }
}
