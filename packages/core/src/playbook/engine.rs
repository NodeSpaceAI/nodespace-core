//! Playbook Engine
//!
//! The top-level engine that subscribes to domain events, matches them against
//! the trigger index, and (in Phase 2+) enqueues work items for processing.
//!
//! Phase 1 scope: subscribe to events, manage lifecycle, match triggers.
//! Phase 2 will add the ExecutionQueue and RuleProcessor.

use crate::db::events::{DomainEvent, EventEnvelope};
use crate::playbook::lifecycle::{trigger_keys_for_event, PlaybookLifecycleManager};
use crate::playbook::types::*;
use crate::services::NodeService;
use std::sync::{Arc, RwLock};
use tokio::sync::broadcast;
use tracing::{debug, error, info, warn};

/// The playbook engine — subscribes to domain events and manages playbook lifecycle.
///
/// Runs in-process alongside NodeService. Subscribes to the broadcast channel
/// as a second subscriber (alongside DomainEventForwarder).
pub struct PlaybookEngine {
    /// Lifecycle manager behind RwLock for concurrent access.
    /// Read: event subscriber (frequent). Write: lifecycle ops (infrequent).
    lifecycle: Arc<RwLock<PlaybookLifecycleManager>>,
    /// NodeService for fetching playbook nodes and (later) executing actions.
    node_service: Arc<NodeService>,
}

impl PlaybookEngine {
    /// Create a new PlaybookEngine.
    ///
    /// Does NOT start the event subscription — call `start()` to begin processing.
    pub fn new(node_service: Arc<NodeService>) -> Self {
        Self {
            lifecycle: Arc::new(RwLock::new(PlaybookLifecycleManager::new())),
            node_service,
        }
    }

    /// Initialize the engine: load active playbooks, build indexes, start event loop.
    ///
    /// Follows the startup sequence from the spec:
    /// 1. Subscribe to event broadcast channel FIRST (to avoid race)
    /// 2. Query all active playbook nodes
    /// 3. Parse rules, build TriggerIndex and CronRegistry
    /// 4. Begin processing events
    ///
    /// The `shutdown_rx` watch channel signals graceful shutdown when it receives `true`.
    pub async fn start(
        self: Arc<Self>,
        mut shutdown_rx: tokio::sync::watch::Receiver<bool>,
    ) -> anyhow::Result<()> {
        // Step 1: Subscribe FIRST to avoid missing events between load and subscribe
        let mut rx = self.node_service.subscribe_to_events();
        info!("Playbook engine subscribed to event channel");

        // Step 2-3: Load active playbooks and build indexes
        self.load_active_playbooks().await?;

        info!("Playbook engine started, processing events...");

        // Step 4: Process events
        loop {
            tokio::select! {
                result = rx.recv() => {
                    match result {
                        Ok(envelope) => {
                            self.handle_event(envelope).await;
                        }
                        Err(broadcast::error::RecvError::Lagged(count)) => {
                            warn!("Playbook engine lagged, missed {} events", count);
                        }
                        Err(broadcast::error::RecvError::Closed) => {
                            info!("Event channel closed, playbook engine shutting down");
                            return Ok(());
                        }
                    }
                }
                _ = shutdown_rx.changed() => {
                    if *shutdown_rx.borrow() {
                        info!("Playbook engine received shutdown signal");
                        return Ok(());
                    }
                }
            }
        }
    }

    /// Load all active playbook nodes from the database and activate them.
    async fn load_active_playbooks(&self) -> anyhow::Result<()> {
        let nodes = self
            .node_service
            .query_nodes_by_type("playbook", Some("active"))
            .await?;

        let mut lifecycle = self.lifecycle.write().expect("lifecycle lock poisoned");
        let mut loaded = 0;
        for node in &nodes {
            match lifecycle.activate_playbook(node) {
                Ok(()) => loaded += 1,
                Err(e) => {
                    warn!("Failed to parse playbook {}: {}", node.id, e);
                }
            }
        }

        info!(
            "Loaded {} active playbooks ({} total found)",
            loaded,
            nodes.len()
        );
        Ok(())
    }

    /// Handle a single event from the broadcast channel.
    ///
    /// Performs lifecycle management (detect playbook CRUD) and trigger matching.
    async fn handle_event(&self, envelope: EventEnvelope) {
        // Lifecycle management: detect playbook node events
        match &envelope.event {
            DomainEvent::NodeCreated { node_type, node_id } if node_type == "playbook" => {
                self.handle_playbook_created(node_id).await;
                return;
            }
            DomainEvent::NodeDeleted { node_type, id } if node_type == "playbook" => {
                self.handle_playbook_deleted(id);
                return;
            }
            DomainEvent::NodeUpdated {
                node_type, node_id, ..
            } if node_type == "playbook" => {
                self.handle_playbook_updated(node_id).await;
                return;
            }
            // Schema version drift detection
            DomainEvent::NodeUpdated {
                node_type, node_id, ..
            } if node_type == "schema" => {
                self.handle_schema_updated(node_id).await;
                return;
            }
            _ => {}
        }

        // Trigger matching for non-lifecycle events
        let keys = trigger_keys_for_event(&envelope.event);
        if keys.is_empty() {
            return;
        }

        let matched_rules = {
            let lifecycle = self.lifecycle.read().expect("lifecycle lock poisoned");
            lifecycle.lookup_rules(&keys)
        };

        if matched_rules.is_empty() {
            return;
        }

        debug!(
            "Event matched {} rules: {:?}",
            matched_rules.len(),
            matched_rules
                .iter()
                .map(|r| format!("{}[{}]", r.playbook_id, r.rule_index))
                .collect::<Vec<_>>()
        );

        // Phase 2 will enqueue these into the ExecutionQueue.
        // For now, we just log the matches.
    }

    /// Handle a new playbook node being created — parse and activate it.
    async fn handle_playbook_created(&self, node_id: &str) {
        match self.node_service.get_node(node_id).await {
            Ok(Some(node)) if node.lifecycle_status == "active" => {
                let mut lifecycle = self.lifecycle.write().expect("lifecycle lock poisoned");
                if let Err(e) = lifecycle.activate_playbook(&node) {
                    warn!("Failed to activate new playbook {}: {}", node_id, e);
                }
            }
            Ok(Some(_)) => {
                debug!("New playbook {} is not active, skipping", node_id);
            }
            Ok(None) => {
                warn!("Playbook {} not found after NodeCreated event", node_id);
            }
            Err(e) => {
                error!("Failed to fetch playbook {}: {}", node_id, e);
            }
        }
    }

    /// Handle a playbook node being deleted — remove from all indexes.
    fn handle_playbook_deleted(&self, playbook_id: &str) {
        let mut lifecycle = self.lifecycle.write().expect("lifecycle lock poisoned");
        lifecycle.deactivate_playbook(playbook_id);
    }

    /// Handle a playbook node being updated — detect status transitions.
    ///
    /// If lifecycle_status changed from disabled→active, re-enable.
    /// If rules changed, re-parse.
    async fn handle_playbook_updated(&self, node_id: &str) {
        match self.node_service.get_node(node_id).await {
            Ok(Some(node)) => {
                let mut lifecycle = self.lifecycle.write().expect("lifecycle lock poisoned");

                let current_status = lifecycle.get_playbook(node_id).map(|pb| pb.status.clone());

                match (current_status, node.lifecycle_status.as_str()) {
                    // Disabled → Active: re-enable
                    (Some(PlaybookStatus::Disabled), "active") => {
                        if let Err(e) = lifecycle.reenable_playbook(&node) {
                            warn!("Failed to re-enable playbook {}: {}", node_id, e);
                        }
                    }
                    // Active → Non-active: disable
                    (Some(PlaybookStatus::Active), status) if status != "active" => {
                        lifecycle.disable_playbook(node_id);
                    }
                    // Active → Active: rules may have changed, re-parse
                    (Some(PlaybookStatus::Active), "active") => {
                        if let Err(e) = lifecycle.reenable_playbook(&node) {
                            warn!("Failed to update playbook {}: {}", node_id, e);
                        }
                    }
                    // Not tracked yet but now active: activate
                    (None, "active") => {
                        if let Err(e) = lifecycle.activate_playbook(&node) {
                            warn!("Failed to activate playbook {}: {}", node_id, e);
                        }
                    }
                    _ => {}
                }
            }
            Ok(None) => {
                warn!("Playbook {} not found after NodeUpdated event", node_id);
            }
            Err(e) => {
                error!("Failed to fetch playbook {}: {}", node_id, e);
            }
        }
    }

    /// Handle a schema node being updated — check for version drift.
    async fn handle_schema_updated(&self, schema_node_id: &str) {
        match self.node_service.get_node(schema_node_id).await {
            Ok(Some(node)) => {
                // Extract schema_node_type and version from the schema node
                let schema_node_type = node
                    .properties
                    .get("schema")
                    .and_then(|s| s.get("forNodeType"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let new_version = node
                    .properties
                    .get("schema")
                    .and_then(|s| s.get("schemaVersion"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("0");

                if schema_node_type.is_empty() {
                    return;
                }

                let mut lifecycle = self.lifecycle.write().expect("lifecycle lock poisoned");
                let disabled = lifecycle.handle_schema_update(schema_node_type, new_version);

                if !disabled.is_empty() {
                    warn!(
                        "Schema drift: {} playbooks disabled due to schema '{}' update: {:?}",
                        disabled.len(),
                        schema_node_type,
                        disabled
                    );
                    // Phase 6 will create log nodes for each disabled playbook
                }
            }
            Ok(None) => {
                debug!(
                    "Schema node {} not found after update event",
                    schema_node_id
                );
            }
            Err(e) => {
                error!("Failed to fetch schema node {}: {}", schema_node_id, e);
            }
        }
    }

    /// Get a snapshot of the lifecycle manager for inspection/testing.
    pub fn lifecycle(&self) -> &Arc<RwLock<PlaybookLifecycleManager>> {
        &self.lifecycle
    }
}
