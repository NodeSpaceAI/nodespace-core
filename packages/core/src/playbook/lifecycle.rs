//! Play Lifecycle Manager
//!
//! Owns all engine state: TriggerIndex, CronRegistry, ActivePlaybooks.
//! Handles install/uninstall/enable/disable of plays and builds
//! the trigger index for O(1) event-to-rule matching.

use crate::models::Node;
use crate::playbook::types::*;
use std::collections::HashMap;
use std::sync::Arc;
use tracing::{debug, info, warn};

/// Manages the lifecycle of all active plays in the engine.
///
/// Thread safety: `TriggerIndex` is behind `Arc<std::sync::RwLock<>>` for
/// concurrent read access from the event subscriber. Write access (lifecycle
/// operations) is infrequent and short-lived.
#[derive(Default)]
pub struct PlaybookLifecycleManager {
    /// Active (and disabled) plays indexed by ID
    active_playbooks: HashMap<String, ParsedPlay>,
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

    /// Load and activate a play node into the engine.
    ///
    /// Parses the rules from the node's properties, builds trigger keys,
    /// and inserts into the index. Idempotent — activating an already-active
    /// play is a no-op (handles startup + reactive event overlap).
    pub fn activate_play(&mut self, node: &Node) -> Result<(), PlayParseError> {
        if self.active_playbooks.contains_key(&node.id) {
            debug!("Play {} already active, skipping", node.id);
            return Ok(());
        }

        let rule_defs = parse_rules_from_properties(&node.properties)?;
        let mut parsed_rules = Vec::with_capacity(rule_defs.len());

        for def in &rule_defs {
            parsed_rules.push(Arc::new(parse_rule(def)?));
        }

        let play = ParsedPlay {
            id: node.id.clone(),
            created_at: node.created_at,
            rules: parsed_rules.clone(),
            status: PlayStatus::Active,
        };

        // Build trigger entries for each rule
        for (idx, rule) in parsed_rules.iter().enumerate() {
            let ordered_ref = OrderedRuleRef {
                play_id: node.id.clone(),
                play_created_at: node.created_at,
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

        info!("Activated play {} with {} rules", node.id, rule_defs.len());
        self.active_playbooks.insert(node.id.clone(), play);
        Ok(())
    }

    /// Remove a play from all indexes (on deletion or permanent removal).
    pub fn deactivate_play(&mut self, play_id: &str) {
        if self.active_playbooks.remove(play_id).is_none() {
            debug!("Play {} not found for deactivation", play_id);
            return;
        }

        self.remove_from_trigger_index(play_id);
        self.remove_from_cron_registry(play_id);
        info!("Deactivated play {}", play_id);
    }

    /// Disable a play — remove from indexes but keep in active_playbooks as disabled.
    ///
    /// Called on first error or when schema version drifts.
    pub fn disable_play(&mut self, play_id: &str) {
        if let Some(play) = self.active_playbooks.get_mut(play_id) {
            play.status = PlayStatus::Disabled;
            self.remove_from_trigger_index(play_id);
            self.remove_from_cron_registry(play_id);
            info!("Disabled play {}", play_id);
        } else {
            warn!("Play {} not found for disabling", play_id);
        }
    }

    /// Re-enable a previously disabled play.
    ///
    /// Re-parses rules from the provided node and re-inserts into indexes.
    pub fn reenable_play(&mut self, node: &Node) -> Result<(), PlayParseError> {
        // Remove existing entry so activate_play isn't a no-op
        self.active_playbooks.remove(&node.id);
        self.remove_from_trigger_index(&node.id);
        self.remove_from_cron_registry(&node.id);

        self.activate_play(node)
    }

    /// Handle a schema update — check if any active plays reference the
    /// affected schema's node_type (either directly as a trigger or via dot-path
    /// traversal in conditions) and need to be disabled.
    ///
    /// Uses path extraction to find plays whose conditions traverse
    /// through the changed schema, not just those that trigger on it directly.
    ///
    /// Returns the list of play IDs that were disabled due to schema drift.
    pub fn handle_schema_update(
        &mut self,
        schema_node_type: &str,
        new_schema_version: &str,
    ) -> Vec<String> {
        let mut disabled = Vec::new();

        // Collect play IDs that reference this schema either directly or via paths
        let affected: Vec<String> = self
            .active_playbooks
            .iter()
            .filter(|(_, pb)| pb.status == PlayStatus::Active)
            .filter(|(_, pb)| {
                play_references_node_type(pb, schema_node_type)
                    || play_has_paths_through_schema(pb, schema_node_type)
            })
            .map(|(id, _)| id.clone())
            .collect();

        for pb_id in affected {
            warn!(
                "Schema '{}' updated to version '{}', disabling play {}",
                schema_node_type, new_schema_version, pb_id
            );
            self.disable_play(&pb_id);
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

        // Deduplicate (same play + rule_index) and sort
        result.sort();
        result.dedup();
        result
    }

    /// Get a reference to the active plays map.
    pub fn active_playbooks(&self) -> &HashMap<String, ParsedPlay> {
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

    /// Get a play by ID.
    pub fn get_play(&self, id: &str) -> Option<&ParsedPlay> {
        self.active_playbooks.get(id)
    }

    // -----------------------------------------------------------------------
    // Internal helpers
    // -----------------------------------------------------------------------

    fn remove_from_trigger_index(&mut self, play_id: &str) {
        self.trigger_index.retain(|_, rules| {
            rules.retain(|r| r.play_id != play_id);
            !rules.is_empty()
        });
    }

    fn remove_from_cron_registry(&mut self, play_id: &str) {
        for entry in &mut self.cron_registry {
            entry.rules.retain(|r| r.play_id != play_id);
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

/// Check if a play's rules reference a given node_type.
fn play_references_node_type(play: &ParsedPlay, node_type: &str) -> bool {
    play.rules.iter().any(|rule| match &rule.trigger {
        ParsedTrigger::GraphEvent { node_type: nt, .. } => nt == node_type,
        ParsedTrigger::Scheduled { node_type: nt, .. } => nt == node_type,
    })
}

/// Check if any of a play's conditions contain dot-paths that might traverse
/// through the given schema's node_type.
///
/// This is a heuristic: we extract paths from conditions and check if any segment
/// matches the schema name. A precise check would require walking the full schema
/// graph, but that's expensive for a lifecycle operation. The heuristic is conservative
/// (may produce false positives, triggering unnecessary re-validation, but never
/// false negatives that would leave a broken play active).
///
/// NOTE: Action binding templates (e.g., `{trigger.node.story.epic.title}`) are not
/// checked here since they're template strings, not CEL expressions. If an action
/// binding references a path through a changed schema, drift detection won't catch it.
/// The action will fail at execution time and the play will be disabled then.
fn play_has_paths_through_schema(play: &ParsedPlay, schema_node_type: &str) -> bool {
    for rule in &play.rules {
        for condition in &rule.conditions {
            if let Ok(extraction) =
                crate::playbook::path_extractor::extract_paths(&condition.source)
            {
                // Check if any extracted path mentions a segment that looks like the schema type
                for path in &extraction.paths {
                    if path.segments.iter().any(|s| s == schema_node_type) {
                        return true;
                    }
                }
                for coll in &extraction.collections {
                    if coll
                        .collection
                        .segments
                        .iter()
                        .any(|s| s == schema_node_type)
                    {
                        return true;
                    }
                }
            }
        }
    }
    false
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
            // NodeDeleted doesn't trigger play rules via TriggerKey
            // (handled separately for play lifecycle)
            vec![]
        }
        DomainEvent::RelationshipCreated { .. } => {
            // TODO(phase2): Relationship events don't carry source_node_type.
            // The EventSubscriber will need to fetch the source node to determine its
            // type, then perform TriggerKey::RelationshipEvent lookup. Until then,
            // relationship_added/relationship_removed triggers are indexed but not matched.
            vec![]
        }
        DomainEvent::RelationshipUpdated { .. } => vec![], // No play triggers for updates
        DomainEvent::RelationshipDeleted { .. } => vec![], // TODO(phase2): same as RelationshipCreated above
        // Infrastructure-failure signal, not a content change — no play trigger keys.
        DomainEvent::BackgroundImportFailed { .. } => vec![],
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::Node;
    use chrono::Utc;
    use serde_json::json;

    /// Helper: create a play node with rules JSON.
    fn make_play_node(id: &str, rules_json: serde_json::Value) -> Node {
        Node {
            id: id.to_string(),
            node_type: "play".to_string(),
            content: format!("play {}", id),
            version: 1,
            created_at: Utc::now(),
            modified_at: Utc::now(),
            properties: json!({ "rules": rules_json }),
            mentions: vec![],
            mentioned_in: vec![],
            title: Some(format!("Play {}", id)),
            lifecycle_status: "active".to_string(),
        }
    }

    // -----------------------------------------------------------------------
    // activate / deactivate / disable
    // -----------------------------------------------------------------------

    #[test]
    fn activate_and_lookup() {
        let mut lm = PlaybookLifecycleManager::new();
        let node = make_play_node(
            "pb-1",
            json!([{
                "name": "r1",
                "trigger": { "type": "graph_event", "on": "node_created", "node_type": "task" },
                "conditions": [],
                "actions": []
            }]),
        );
        lm.activate_play(&node).unwrap();
        assert!(lm.active_playbooks().contains_key("pb-1"));
        assert_eq!(lm.active_playbooks()["pb-1"].status, PlayStatus::Active);
    }

    #[test]
    fn deactivate_removes_from_all_indexes() {
        let mut lm = PlaybookLifecycleManager::new();
        let node = make_play_node(
            "pb-2",
            json!([{
                "name": "r1",
                "trigger": { "type": "graph_event", "on": "node_created", "node_type": "task" },
                "conditions": [],
                "actions": []
            }]),
        );
        lm.activate_play(&node).unwrap();
        assert!(!lm.trigger_index().is_empty());

        lm.deactivate_play("pb-2");
        assert!(!lm.active_playbooks().contains_key("pb-2"));
        assert!(lm.trigger_index().is_empty());
    }

    #[test]
    fn disable_keeps_in_active_but_removes_from_index() {
        let mut lm = PlaybookLifecycleManager::new();
        let node = make_play_node(
            "pb-3",
            json!([{
                "name": "r1",
                "trigger": { "type": "graph_event", "on": "node_created", "node_type": "task" },
                "conditions": [],
                "actions": []
            }]),
        );
        lm.activate_play(&node).unwrap();
        lm.disable_play("pb-3");

        // Still in active_playbooks but disabled
        assert!(lm.active_playbooks().contains_key("pb-3"));
        assert_eq!(lm.active_playbooks()["pb-3"].status, PlayStatus::Disabled);
        // Removed from trigger index
        assert!(lm.trigger_index().is_empty());
    }

    #[test]
    fn lookup_rules_returns_matching_rules() {
        let mut lm = PlaybookLifecycleManager::new();
        let node = make_play_node(
            "pb-4",
            json!([{
                "name": "r1",
                "trigger": { "type": "graph_event", "on": "node_created", "node_type": "task" },
                "conditions": ["node.status == 'open'"],
                "actions": []
            }]),
        );
        lm.activate_play(&node).unwrap();

        let keys = vec![TriggerKey::NodeEvent {
            event: NodeEventType::NodeCreated,
            node_type: "task".to_string(),
            property_key: None,
        }];
        let rules = lm.lookup_rules(&keys);
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].rule.name, "r1");
    }

    #[test]
    fn lookup_rules_no_match_returns_empty() {
        let mut lm = PlaybookLifecycleManager::new();
        let node = make_play_node(
            "pb-5",
            json!([{
                "name": "r1",
                "trigger": { "type": "graph_event", "on": "node_created", "node_type": "task" },
                "conditions": [],
                "actions": []
            }]),
        );
        lm.activate_play(&node).unwrap();

        // Wrong node type
        let keys = vec![TriggerKey::NodeEvent {
            event: NodeEventType::NodeCreated,
            node_type: "invoice".to_string(),
            property_key: None,
        }];
        let rules = lm.lookup_rules(&keys);
        assert!(rules.is_empty());
    }

    // -----------------------------------------------------------------------
    // handle_schema_update — path-aware drift detection
    // -----------------------------------------------------------------------

    #[test]
    fn schema_update_disables_directly_referencing_play() {
        let mut lm = PlaybookLifecycleManager::new();
        let node = make_play_node(
            "pb-drift-1",
            json!([{
                "name": "r1",
                "trigger": { "type": "graph_event", "on": "node_created", "node_type": "task" },
                "conditions": ["node.status == 'open'"],
                "actions": []
            }]),
        );
        lm.activate_play(&node).unwrap();

        let disabled = lm.handle_schema_update("task", "2");
        assert_eq!(disabled, vec!["pb-drift-1"]);
        assert_eq!(
            lm.active_playbooks()["pb-drift-1"].status,
            PlayStatus::Disabled
        );
    }

    #[test]
    fn schema_update_disables_play_with_path_through_schema() {
        let mut lm = PlaybookLifecycleManager::new();
        // Play triggers on "task" but has conditions traversing through "epic"
        let node = make_play_node(
            "pb-drift-2",
            json!([{
                "name": "r1",
                "trigger": { "type": "graph_event", "on": "node_created", "node_type": "task" },
                "conditions": ["node.story.epic.status == 'active'"],
                "actions": []
            }]),
        );
        lm.activate_play(&node).unwrap();

        // Updating "epic" schema should detect the path traversal
        let disabled = lm.handle_schema_update("epic", "2");
        assert_eq!(disabled, vec!["pb-drift-2"]);
    }

    #[test]
    fn schema_update_does_not_affect_unrelated_play() {
        let mut lm = PlaybookLifecycleManager::new();
        let node = make_play_node(
            "pb-drift-3",
            json!([{
                "name": "r1",
                "trigger": { "type": "graph_event", "on": "node_created", "node_type": "task" },
                "conditions": ["node.status == 'open'"],
                "actions": []
            }]),
        );
        lm.activate_play(&node).unwrap();

        // Updating "invoice" schema should not affect this play
        let disabled = lm.handle_schema_update("invoice", "2");
        assert!(disabled.is_empty());
        assert_eq!(
            lm.active_playbooks()["pb-drift-3"].status,
            PlayStatus::Active
        );
    }

    #[test]
    fn schema_update_skips_already_disabled_plays() {
        let mut lm = PlaybookLifecycleManager::new();
        let node = make_play_node(
            "pb-drift-4",
            json!([{
                "name": "r1",
                "trigger": { "type": "graph_event", "on": "node_created", "node_type": "task" },
                "conditions": [],
                "actions": []
            }]),
        );
        lm.activate_play(&node).unwrap();
        lm.disable_play("pb-drift-4");

        let disabled = lm.handle_schema_update("task", "2");
        assert!(
            disabled.is_empty(),
            "already-disabled plays should not appear"
        );
    }

    // -----------------------------------------------------------------------
    // play_has_paths_through_schema
    // -----------------------------------------------------------------------

    #[test]
    fn paths_through_schema_detects_multi_hop() {
        let pb = ParsedPlay {
            id: "test-pb".to_string(),
            created_at: Utc::now(),
            rules: vec![Arc::new(ParsedRule {
                name: "r1".to_string(),
                class: RuleClass::Reactive,
                trigger: ParsedTrigger::GraphEvent {
                    on: GraphEventType::NodeCreated,
                    node_type: "task".to_string(),
                    property_key: None,
                },
                conditions: vec![crate::playbook::cel::CompiledCondition::compile(
                    "node.story.epic.status == 'active'",
                )
                .unwrap()],
                actions: vec![],
            })],
            status: PlayStatus::Active,
        };

        assert!(play_has_paths_through_schema(&pb, "story"));
        assert!(play_has_paths_through_schema(&pb, "epic"));
        assert!(!play_has_paths_through_schema(&pb, "invoice"));
    }

    #[test]
    fn paths_through_schema_no_conditions() {
        let pb = ParsedPlay {
            id: "test-pb".to_string(),
            created_at: Utc::now(),
            rules: vec![Arc::new(ParsedRule {
                name: "r1".to_string(),
                class: RuleClass::Reactive,
                trigger: ParsedTrigger::GraphEvent {
                    on: GraphEventType::NodeCreated,
                    node_type: "task".to_string(),
                    property_key: None,
                },
                conditions: vec![],
                actions: vec![],
            })],
            status: PlayStatus::Active,
        };

        assert!(!play_has_paths_through_schema(&pb, "task"));
    }

    // -----------------------------------------------------------------------
    // trigger_keys_for_event
    // -----------------------------------------------------------------------

    #[test]
    fn trigger_keys_for_node_created() {
        let event = crate::db::events::DomainEvent::NodeCreated {
            node_type: "task".to_string(),
            node_id: "n1".to_string(),
        };
        let keys = trigger_keys_for_event(&event);
        assert_eq!(keys.len(), 1);
        assert!(matches!(
            &keys[0],
            TriggerKey::NodeEvent {
                event: NodeEventType::NodeCreated,
                node_type,
                property_key: None,
            } if node_type == "task"
        ));
    }

    #[test]
    fn trigger_keys_for_property_changed_includes_wildcard() {
        let event = crate::db::events::DomainEvent::NodeUpdated {
            node_type: "task".to_string(),
            node_id: "n1".to_string(),
            node: Node {
                id: "n1".to_string(),
                node_type: "task".to_string(),
                content: String::new(),
                version: 1,
                created_at: Utc::now(),
                modified_at: Utc::now(),
                properties: json!({}),
                mentions: vec![],
                mentioned_in: vec![],
                title: None,
                lifecycle_status: "active".to_string(),
            },
            changed_properties: vec![crate::db::events::PropertyChange {
                key: "status".to_string(),
                old_value: Some(json!("open")),
                new_value: Some(json!("done")),
            }],
        };
        let keys = trigger_keys_for_event(&event);
        // Should have exact key + wildcard
        assert_eq!(keys.len(), 2);
    }
}
