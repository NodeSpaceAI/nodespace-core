//! Playbook Lifecycle Manager
//!
//! Owns all engine state: TriggerIndex, CronRegistry, ActivePlaybooks.
//! Handles install/uninstall/enable/disable of playbooks and builds
//! the trigger index for O(1) event-to-rule matching.

use crate::models::Node;
use crate::playbook::types::*;
use std::collections::HashMap;
use std::sync::Arc;
use tracing::{debug, info, warn};

/// Manages the lifecycle of all active playbooks in the engine.
///
/// Thread safety: `TriggerIndex` is behind `Arc<std::sync::RwLock<>>` for
/// concurrent read access from the event subscriber. Write access (lifecycle
/// operations) is infrequent and short-lived.
#[derive(Default)]
pub struct PlaybookLifecycleManager {
    /// Active (and disabled) playbooks indexed by ID
    active_playbooks: HashMap<String, ParsedPlaybook>,
    /// Trigger index for O(1) event → rules lookup
    trigger_index: TriggerIndex,
    /// Cron registry for scheduled triggers
    cron_registry: CronRegistry,
}

impl PlaybookLifecycleManager {
    pub fn new() -> Self {
        Self {
            active_playbooks: HashMap::new(),
            trigger_index: HashMap::new(),
            cron_registry: Vec::new(),
        }
    }

    /// Load and activate a playbook node into the engine.
    ///
    /// Parses the rules from the node's properties, builds trigger keys,
    /// and inserts into the index. Idempotent — activating an already-active
    /// playbook is a no-op (handles startup + reactive event overlap).
    pub fn activate_playbook(&mut self, node: &Node) -> Result<(), PlaybookParseError> {
        if self.active_playbooks.contains_key(&node.id) {
            debug!("Playbook {} already active, skipping", node.id);
            return Ok(());
        }

        let rule_defs = parse_rules_from_properties(&node.properties)?;
        let mut parsed_rules = Vec::with_capacity(rule_defs.len());

        for def in &rule_defs {
            parsed_rules.push(Arc::new(parse_rule(def)?));
        }

        let playbook = ParsedPlaybook {
            id: node.id.clone(),
            created_at: node.created_at,
            rules: parsed_rules.clone(),
            status: PlaybookStatus::Active,
        };

        // Build trigger entries for each rule
        for (idx, rule) in parsed_rules.iter().enumerate() {
            let ordered_ref = OrderedRuleRef {
                playbook_id: node.id.clone(),
                playbook_created_at: node.created_at,
                rule_index: idx,
                rule: Arc::clone(rule),
            };

            match &rule.trigger {
                ParsedTrigger::GraphEvent {
                    on,
                    node_type,
                    property_key,
                } => {
                    let keys = trigger_keys_for_graph_event(on, node_type, property_key.as_deref());
                    for key in keys {
                        let entries = self.trigger_index.entry(key).or_default();
                        entries.push(ordered_ref.clone());
                        entries.sort();
                    }
                }
                ParsedTrigger::Scheduled { cron, node_type } => {
                    // Find existing cron entry or create new one
                    if let Some(entry) = self
                        .cron_registry
                        .iter_mut()
                        .find(|e| e.cron_expression == *cron && e.node_type == *node_type)
                    {
                        entry.rules.push(ordered_ref);
                        entry.rules.sort();
                    } else {
                        self.cron_registry.push(CronEntry {
                            cron_expression: cron.clone(),
                            node_type: node_type.clone(),
                            rules: vec![ordered_ref],
                        });
                    }
                }
            }
        }

        info!(
            "Activated playbook {} with {} rules",
            node.id,
            rule_defs.len()
        );
        self.active_playbooks.insert(node.id.clone(), playbook);
        Ok(())
    }

    /// Remove a playbook from all indexes (on deletion or permanent removal).
    pub fn deactivate_playbook(&mut self, playbook_id: &str) {
        if self.active_playbooks.remove(playbook_id).is_none() {
            debug!("Playbook {} not found for deactivation", playbook_id);
            return;
        }

        self.remove_from_trigger_index(playbook_id);
        self.remove_from_cron_registry(playbook_id);
        info!("Deactivated playbook {}", playbook_id);
    }

    /// Disable a playbook — remove from indexes but keep in active_playbooks as disabled.
    ///
    /// Called on first error or when schema version drifts.
    pub fn disable_playbook(&mut self, playbook_id: &str) {
        if let Some(playbook) = self.active_playbooks.get_mut(playbook_id) {
            playbook.status = PlaybookStatus::Disabled;
            self.remove_from_trigger_index(playbook_id);
            self.remove_from_cron_registry(playbook_id);
            info!("Disabled playbook {}", playbook_id);
        } else {
            warn!("Playbook {} not found for disabling", playbook_id);
        }
    }

    /// Re-enable a previously disabled playbook.
    ///
    /// Re-parses rules from the provided node and re-inserts into indexes.
    pub fn reenable_playbook(&mut self, node: &Node) -> Result<(), PlaybookParseError> {
        // Remove existing entry so activate_playbook isn't a no-op
        self.active_playbooks.remove(&node.id);
        self.remove_from_trigger_index(&node.id);
        self.remove_from_cron_registry(&node.id);

        self.activate_playbook(node)
    }

    /// Handle a schema update — check if any active playbooks reference the
    /// affected schema's node_type and need to be disabled.
    ///
    /// Returns the list of playbook IDs that were disabled due to version drift.
    pub fn handle_schema_update(
        &mut self,
        schema_node_type: &str,
        new_schema_version: &str,
    ) -> Vec<String> {
        let mut disabled = Vec::new();

        // Collect playbook IDs that reference this schema
        let affected: Vec<String> = self
            .active_playbooks
            .iter()
            .filter(|(_, pb)| pb.status == PlaybookStatus::Active)
            .filter(|(_, pb)| playbook_references_node_type(pb, schema_node_type))
            .map(|(id, _)| id.clone())
            .collect();

        for pb_id in affected {
            // Check if the playbook's version references match the new version.
            // For now, any schema change to a referenced type disables the playbook.
            // Phase 7 (save-time validation) will add version-aware checking.
            warn!(
                "Schema '{}' updated to version '{}', disabling playbook {}",
                schema_node_type, new_schema_version, pb_id
            );
            self.disable_playbook(&pb_id);
            disabled.push(pb_id);
        }

        disabled
    }

    /// Lookup rules matching a set of trigger keys.
    ///
    /// For `PropertyChanged` events, the caller should provide both the exact
    /// key and the wildcard key. Results are merged and deduplicated.
    pub fn lookup_rules(&self, keys: &[TriggerKey]) -> Vec<OrderedRuleRef> {
        let mut result: Vec<OrderedRuleRef> = Vec::new();

        for key in keys {
            if let Some(rules) = self.trigger_index.get(key) {
                result.extend(rules.iter().cloned());
            }
        }

        // Deduplicate (same playbook + rule_index) and sort
        result.sort();
        result.dedup();
        result
    }

    /// Get a reference to the active playbooks map.
    pub fn active_playbooks(&self) -> &HashMap<String, ParsedPlaybook> {
        &self.active_playbooks
    }

    /// Get a reference to the trigger index.
    pub fn trigger_index(&self) -> &TriggerIndex {
        &self.trigger_index
    }

    /// Get a reference to the cron registry.
    pub fn cron_registry(&self) -> &CronRegistry {
        &self.cron_registry
    }

    /// Get a playbook by ID.
    pub fn get_playbook(&self, id: &str) -> Option<&ParsedPlaybook> {
        self.active_playbooks.get(id)
    }

    // -----------------------------------------------------------------------
    // Internal helpers
    // -----------------------------------------------------------------------

    fn remove_from_trigger_index(&mut self, playbook_id: &str) {
        self.trigger_index.retain(|_, rules| {
            rules.retain(|r| r.playbook_id != playbook_id);
            !rules.is_empty()
        });
    }

    fn remove_from_cron_registry(&mut self, playbook_id: &str) {
        for entry in &mut self.cron_registry {
            entry.rules.retain(|r| r.playbook_id != playbook_id);
        }
        self.cron_registry.retain(|e| !e.rules.is_empty());
    }
}

/// Build trigger keys for a graph event trigger definition.
///
/// For `PropertyChanged` with a specific property_key, creates TWO keys:
/// one exact and one wildcard (None). This allows wildcard rules to match
/// all property changes on a node type.
fn trigger_keys_for_graph_event(
    on: &GraphEventType,
    node_type: &str,
    property_key: Option<&str>,
) -> Vec<TriggerKey> {
    match on {
        GraphEventType::NodeCreated => vec![TriggerKey::NodeEvent {
            event: NodeEventType::NodeCreated,
            node_type: node_type.to_string(),
            property_key: None,
        }],
        GraphEventType::PropertyChanged => {
            // Insert under the specific property_key (or None for wildcard rules)
            vec![TriggerKey::NodeEvent {
                event: NodeEventType::PropertyChanged,
                node_type: node_type.to_string(),
                property_key: property_key.map(|s| s.to_string()),
            }]
        }
        GraphEventType::RelationshipAdded => vec![TriggerKey::RelationshipEvent {
            event: RelEventType::RelationshipAdded,
            source_node_type: node_type.to_string(),
        }],
        GraphEventType::RelationshipRemoved => vec![TriggerKey::RelationshipEvent {
            event: RelEventType::RelationshipRemoved,
            source_node_type: node_type.to_string(),
        }],
    }
}

/// Check if a playbook's rules reference a given node_type.
fn playbook_references_node_type(playbook: &ParsedPlaybook, node_type: &str) -> bool {
    playbook.rules.iter().any(|rule| match &rule.trigger {
        ParsedTrigger::GraphEvent { node_type: nt, .. } => nt == node_type,
        ParsedTrigger::Scheduled { node_type: nt, .. } => nt == node_type,
    })
}

/// Build trigger keys from a domain event for lookup purposes.
///
/// Given event details, produces the set of TriggerKeys to look up in the index.
/// For `PropertyChanged`, returns both exact-key and wildcard-key lookups.
pub fn trigger_keys_for_event(event: &crate::db::events::DomainEvent) -> Vec<TriggerKey> {
    use crate::db::events::DomainEvent;

    match event {
        DomainEvent::NodeCreated { node_type, .. } => {
            vec![TriggerKey::NodeEvent {
                event: NodeEventType::NodeCreated,
                node_type: node_type.clone(),
                property_key: None,
            }]
        }
        DomainEvent::NodeUpdated {
            node_type,
            changed_properties,
            ..
        } => {
            let mut keys = Vec::new();

            // For each changed property, look up exact key AND wildcard
            for prop in changed_properties {
                // Exact property key match
                keys.push(TriggerKey::NodeEvent {
                    event: NodeEventType::PropertyChanged,
                    node_type: node_type.clone(),
                    property_key: Some(prop.key.clone()),
                });
            }

            // Wildcard: any property change on this node type
            if !changed_properties.is_empty() {
                keys.push(TriggerKey::NodeEvent {
                    event: NodeEventType::PropertyChanged,
                    node_type: node_type.clone(),
                    property_key: None,
                });
            }

            keys
        }
        DomainEvent::NodeDeleted { .. } => {
            // NodeDeleted doesn't trigger playbook rules via TriggerKey
            // (handled separately for playbook lifecycle)
            vec![]
        }
        DomainEvent::RelationshipCreated { .. } => {
            // TODO(#995-phase2): Relationship events don't carry source_node_type.
            // The EventSubscriber will need to fetch the source node to determine its
            // type, then perform TriggerKey::RelationshipEvent lookup. Until then,
            // relationship_added/relationship_removed triggers are indexed but not matched.
            vec![]
        }
        DomainEvent::RelationshipUpdated { .. } => vec![], // No playbook triggers for updates
        DomainEvent::RelationshipDeleted { .. } => vec![], // TODO(#995-phase2): same as RelationshipCreated above
    }
}
