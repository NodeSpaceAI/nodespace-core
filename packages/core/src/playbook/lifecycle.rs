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
    /// node_type → its full `extends` ancestry, nearest first (ADR-078).
    ///
    /// A Play registered against a base type must fire for events carrying a
    /// type that extends it, which means resolving the event type's ancestry
    /// before looking up the trigger index. That resolution is a SQL walk, and
    /// this index is consulted on every graph mutation system-wide, in-memory,
    /// with zero I/O — so the ancestry is cached here rather than queried per
    /// event.
    ///
    /// Populated from `engine.rs`, which has store access; this struct
    /// deliberately has none. Empty until something declares `extends`, and an
    /// absent entry means "no ancestry", so an unextended type costs one
    /// failed hash lookup.
    ancestor_cache: HashMap<String, Vec<String>>,
}

impl PlaybookLifecycleManager {
    pub fn new() -> Self {
        Self {
            active_playbooks: HashMap::new(),
            trigger_index: HashMap::new(),
            cron_registry: Vec::new(),
            ancestor_cache: HashMap::new(),
        }
    }

    /// Replace the `extends` ancestry cache (ADR-078).
    ///
    /// Called from the engine on startup and whenever a schema write may have
    /// changed an `extends` edge. Wholesale replacement rather than fine-
    /// grained diffing is proportionate: `extends` edits are rare,
    /// administrative operations, and the map holds one entry per extending
    /// type.
    pub fn set_ancestor_cache(&mut self, cache: HashMap<String, Vec<String>>) {
        self.ancestor_cache = cache;
    }

    /// A node type's cached ancestry, nearest first, including the type itself.
    ///
    /// Falls back to just the type when nothing is cached for it — which is
    /// every type in a database where nothing declares `extends`, and the
    /// correct answer there.
    pub fn ancestors_of(&self, node_type: &str) -> Vec<String> {
        self.ancestor_cache
            .get(node_type)
            .cloned()
            .unwrap_or_else(|| vec![node_type.to_string()])
    }

    /// Whether the ancestry cache holds anything at all.
    ///
    /// Lets the hot path skip ancestor fan-out entirely in the common case.
    pub fn has_ancestry(&self) -> bool {
        !self.ancestor_cache.is_empty()
    }

    /// The same trigger key re-keyed under each *strict* ancestor of its node
    /// type — the type itself is excluded, since the caller has already looked
    /// that one up directly.
    ///
    /// Both halves of the key are rewritten: the node type *and* the
    /// type-namespaced property key, which travel together. See
    /// [`renamespace_property_key`].
    fn ancestor_keys(&self, key: &TriggerKey) -> Vec<TriggerKey> {
        let node_type = match key {
            TriggerKey::NodeEvent { node_type, .. } => node_type,
            TriggerKey::RelationshipEvent {
                source_node_type, ..
            } => source_node_type,
        };

        let Some(chain) = self.ancestor_cache.get(node_type) else {
            return Vec::new();
        };

        chain
            .iter()
            .filter(|ancestor| *ancestor != node_type)
            .map(|ancestor| match key {
                TriggerKey::NodeEvent {
                    event,
                    property_key,
                    ..
                } => TriggerKey::NodeEvent {
                    event: event.clone(),
                    node_type: ancestor.clone(),
                    // Re-namespace the property key to the ancestor as well.
                    // A real `PropertyChanged` event carries a type-namespaced
                    // key (`bug.status`), so carrying it through verbatim
                    // produces `(task, "bug.status")` — which no Play
                    // registered on `task` can ever match, since it registered
                    // `(task, "task.status")`. Rewriting only `node_type`
                    // silently defeats the whole fan-out for exactly the
                    // property-changed triggers it exists to serve.
                    property_key: property_key
                        .as_deref()
                        .map(|k| renamespace_property_key(k, node_type, ancestor)),
                },
                TriggerKey::RelationshipEvent { event, .. } => TriggerKey::RelationshipEvent {
                    event: event.clone(),
                    source_node_type: ancestor.clone(),
                },
            })
            .collect()
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

    /// Plays that *reference* the changed schema — candidates for drift, not
    /// yet known to be broken by it.
    ///
    /// Referencing a type is not the same as being broken by a change to it: a
    /// Play triggering on `task` is untouched when `task` gains a status value,
    /// and disabling it there would silently stop core automation for an
    /// [ADR-076]-blessed extension. Callers resolve candidates to actual
    /// breakage by re-validating against the new schema, which requires store
    /// access this lock-held method deliberately does not take.
    pub fn plays_referencing_schema(&self, schema_node_type: &str) -> Vec<String> {
        self.active_playbooks
            .iter()
            .filter(|(_, pb)| pb.status == PlayStatus::Active)
            .filter(|(_, pb)| {
                play_references_node_type(pb, schema_node_type)
                    || play_has_paths_through_schema(pb, schema_node_type)
            })
            .map(|(id, _)| id.clone())
            .collect()
    }

    /// The parsed rules of one active play, for re-validation after a schema
    /// change.
    pub fn rules_for_play(&self, play_id: &str) -> Option<Vec<Arc<ParsedRule>>> {
        self.active_playbooks
            .get(play_id)
            .map(|pb| pb.rules.clone())
    }

    /// Lookup rules matching a set of trigger keys.
    ///
    /// For `PropertyChanged` events, the caller should provide both the exact
    /// key and the wildcard key. Results are merged and deduplicated.
    ///
    /// Subtype-aware (ADR-078): each key is also looked up under every
    /// ancestor of its node type, so a Play registered against `task` fires on
    /// an event carrying `issue`. Matching stays entirely in memory — the
    /// ancestry comes from this struct's cache, not a query — so the hot path
    /// keeps its zero-I/O profile. Plays registered against exact, unextended
    /// types are unaffected: their ancestry is just themselves.
    pub fn lookup_rules(&self, keys: &[TriggerKey]) -> Vec<OrderedRuleRef> {
        let mut result: Vec<OrderedRuleRef> = Vec::new();

        for key in keys {
            if let Some(rules) = self.trigger_index.get(key) {
                result.extend(rules.iter().cloned());
            }

            // Fan out to the ancestry only when something extends something;
            // otherwise every event would pay for a clone and a re-key.
            if !self.has_ancestry() {
                continue;
            }
            for ancestor_key in self.ancestor_keys(key) {
                if let Some(rules) = self.trigger_index.get(&ancestor_key) {
                    result.extend(rules.iter().cloned());
                }
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
/// Re-point a type-namespaced property key from one type to another.
///
/// A `PropertyChanged` event's key is `<node_type>.<field>` (`bug.status`),
/// matching the stored bucket shape. When a subtype's event fans out to an
/// ancestor's trigger key, the namespace must move with it — a Play registered
/// on `task` indexed itself under `task.status`, not `bug.status`.
///
/// A bare key (no dot) has no namespace to move and is returned unchanged:
/// wildcard/`None` keys and any bare spelling still match as they did. Only the
/// leading segment is replaced, so a field name containing a dot keeps its tail.
fn renamespace_property_key(key: &str, from_type: &str, to_type: &str) -> String {
    match key.split_once('.') {
        Some((namespace, field)) if namespace == from_type => {
            namespaced_property_key(to_type, field)
        }
        _ => key.to_string(),
    }
}

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
        make_play_node_at(id, rules_json, Utc::now())
    }

    /// Helper: create a play node with rules JSON and an explicit `created_at`,
    /// so tests can pit wall-clock creation order against play-id order.
    fn make_play_node_at(
        id: &str,
        rules_json: serde_json::Value,
        created_at: chrono::DateTime<Utc>,
    ) -> Node {
        Node {
            id: id.to_string(),
            node_type: "play".to_string(),
            content: format!("play {}", id),
            version: 1,
            created_at,
            modified_at: created_at,
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
    // Cross-play ordering (ADR-060 §5) — keyed on play id, not created_at
    // -----------------------------------------------------------------------

    #[test]
    fn cross_play_ordering_keys_on_play_id_not_created_at() {
        let mut lm = PlaybookLifecycleManager::new();

        let earlier = "2024-01-01T00:00:00Z"
            .parse::<chrono::DateTime<Utc>>()
            .unwrap();
        let later = "2024-06-01T00:00:00Z"
            .parse::<chrono::DateTime<Utc>>()
            .unwrap();

        // "pb-zebra" is created FIRST (earlier wall-clock time) but sorts
        // LAST lexically. "pb-apple" is created SECOND but sorts first.
        // Under the old (play_created_at, rule_index) key this would order
        // [pb-zebra, pb-apple]; under the play_id key it must order
        // [pb-apple, pb-zebra].
        let zebra = make_play_node_at(
            "pb-zebra",
            json!([{
                "name": "r1",
                "trigger": { "type": "graph_event", "on": "node_created", "node_type": "task" },
                "conditions": [],
                "actions": []
            }]),
            earlier,
        );
        let apple = make_play_node_at(
            "pb-apple",
            json!([{
                "name": "r1",
                "trigger": { "type": "graph_event", "on": "node_created", "node_type": "task" },
                "conditions": [],
                "actions": []
            }]),
            later,
        );

        lm.activate_play(&zebra).unwrap();
        lm.activate_play(&apple).unwrap();

        let keys = vec![TriggerKey::NodeEvent {
            event: NodeEventType::NodeCreated,
            node_type: "task".to_string(),
            property_key: None,
        }];
        let rules = lm.lookup_rules(&keys);

        assert_eq!(
            rules.iter().map(|r| r.play_id.as_str()).collect::<Vec<_>>(),
            vec!["pb-apple", "pb-zebra"],
            "cross-play order must follow play_id, not creation time"
        );
    }

    #[test]
    fn two_devices_different_install_order_produce_same_rule_order() {
        // Simulate two devices that installed the same two plays in opposite
        // order and whose clocks disagree about which play was created first.
        // Per ADR-060 §5, both must evaluate a shared trigger's matched rules
        // in the same order.
        let t_early = "2024-01-01T00:00:00Z"
            .parse::<chrono::DateTime<Utc>>()
            .unwrap();
        let t_late = "2024-06-01T00:00:00Z"
            .parse::<chrono::DateTime<Utc>>()
            .unwrap();

        let rules_json = json!([{
            "name": "r1",
            "trigger": { "type": "graph_event", "on": "node_created", "node_type": "task" },
            "conditions": [],
            "actions": []
        }]);

        // Device A: clock says apple was created before zebra; installs apple then zebra.
        let apple_device_a = make_play_node_at("pb-apple", rules_json.clone(), t_early);
        let zebra_device_a = make_play_node_at("pb-zebra", rules_json.clone(), t_late);
        let mut device_a = PlaybookLifecycleManager::new();
        device_a.activate_play(&apple_device_a).unwrap();
        device_a.activate_play(&zebra_device_a).unwrap();

        // Device B: clock disagrees (zebra looks earlier here) and installs
        // in the opposite order: zebra then apple.
        let zebra_device_b = make_play_node_at("pb-zebra", rules_json.clone(), t_early);
        let apple_device_b = make_play_node_at("pb-apple", rules_json, t_late);
        let mut device_b = PlaybookLifecycleManager::new();
        device_b.activate_play(&zebra_device_b).unwrap();
        device_b.activate_play(&apple_device_b).unwrap();

        let keys = vec![TriggerKey::NodeEvent {
            event: NodeEventType::NodeCreated,
            node_type: "task".to_string(),
            property_key: None,
        }];

        let order_a: Vec<String> = device_a
            .lookup_rules(&keys)
            .into_iter()
            .map(|r| r.play_id)
            .collect();
        let order_b: Vec<String> = device_b
            .lookup_rules(&keys)
            .into_iter()
            .map(|r| r.play_id)
            .collect();

        assert_eq!(
            order_a, order_b,
            "devices with different install order/clocks must agree on rule order"
        );
        assert_eq!(
            order_a,
            vec!["pb-apple".to_string(), "pb-zebra".to_string()]
        );
    }

    #[test]
    fn within_play_rule_order_by_rule_index_is_unchanged() {
        // A single play with three rules on the same trigger must still be
        // evaluated in array (rule_index) order — unaffected by the switch
        // to play_id-keyed cross-play ordering.
        let mut lm = PlaybookLifecycleManager::new();
        let node = make_play_node(
            "pb-multi",
            json!([
                {
                    "name": "third",
                    "trigger": { "type": "graph_event", "on": "node_created", "node_type": "task" },
                    "conditions": [],
                    "actions": []
                },
                {
                    "name": "first",
                    "trigger": { "type": "graph_event", "on": "node_created", "node_type": "task" },
                    "conditions": [],
                    "actions": []
                },
                {
                    "name": "second",
                    "trigger": { "type": "graph_event", "on": "node_created", "node_type": "task" },
                    "conditions": [],
                    "actions": []
                }
            ]),
        );
        lm.activate_play(&node).unwrap();

        let keys = vec![TriggerKey::NodeEvent {
            event: NodeEventType::NodeCreated,
            node_type: "task".to_string(),
            property_key: None,
        }];
        let rules = lm.lookup_rules(&keys);

        // rule_index order (array order), not name order.
        assert_eq!(
            rules
                .iter()
                .map(|r| r.rule.name.as_str())
                .collect::<Vec<_>>(),
            vec!["third", "first", "second"]
        );
        assert_eq!(
            rules.iter().map(|r| r.rule_index).collect::<Vec<_>>(),
            vec![0, 1, 2]
        );
    }

    // -----------------------------------------------------------------------
    // plays_referencing_schema — path-aware drift CANDIDATE detection
    // -----------------------------------------------------------------------

    #[test]
    fn schema_change_flags_directly_referencing_play_as_a_candidate() {
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

        // A candidate, not a verdict: the engine re-validates each against the
        // new schema and disables only those that actually broke. Referencing a
        // type an ADDITIVE change touched must not disable anything.
        let candidates = lm.plays_referencing_schema("task");
        assert_eq!(candidates, vec!["pb-drift-1"]);
        assert_eq!(
            lm.active_playbooks()["pb-drift-1"].status,
            PlayStatus::Active,
            "identifying a candidate must not itself disable it"
        );
    }

    #[test]
    fn schema_change_flags_a_play_whose_path_traverses_the_schema() {
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
        let candidates = lm.plays_referencing_schema("epic");
        assert_eq!(candidates, vec!["pb-drift-2"]);
    }

    #[test]
    fn schema_change_ignores_an_unrelated_play() {
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
        let candidates = lm.plays_referencing_schema("invoice");
        assert!(candidates.is_empty());
        assert_eq!(
            lm.active_playbooks()["pb-drift-3"].status,
            PlayStatus::Active
        );
    }

    #[test]
    fn schema_change_skips_already_disabled_plays() {
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

        let candidates = lm.plays_referencing_schema("task");
        assert!(
            candidates.is_empty(),
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

    // ========================================================================
    // extends — subtype-aware trigger matching (ADR-078)
    // ========================================================================

    /// A manager with one Play triggering on `node_created` for `node_type`.
    fn manager_with_play_on(node_type: &str) -> PlaybookLifecycleManager {
        let mut lm = PlaybookLifecycleManager::new();
        let node = make_play_node(
            "pb-base",
            json!([{
                "name": "r1",
                "trigger": { "type": "graph_event", "on": "node_created", "node_type": node_type },
                "conditions": [],
                "actions": []
            }]),
        );
        lm.activate_play(&node)
            .expect("play activation should succeed");
        lm
    }

    fn node_created_key(node_type: &str) -> TriggerKey {
        TriggerKey::NodeEvent {
            event: NodeEventType::NodeCreated,
            node_type: node_type.to_string(),
            property_key: None,
        }
    }

    #[test]
    fn base_scoped_play_fires_on_a_subtype_event() {
        let mut lm = manager_with_play_on("task");
        lm.set_ancestor_cache(HashMap::from([(
            "issue".to_string(),
            vec!["issue".to_string(), "task".to_string()],
        )]));

        // The event carries the concrete type; the Play was registered against
        // the base. Without ancestry fan-out this returns nothing.
        let rules = lm.lookup_rules(&[node_created_key("issue")]);
        assert_eq!(
            rules.len(),
            1,
            "a Play on 'task' should match an 'issue' event"
        );
    }

    #[test]
    fn base_scoped_play_fires_through_a_transitive_chain() {
        let mut lm = manager_with_play_on("task");
        lm.set_ancestor_cache(HashMap::from([(
            "bug".to_string(),
            vec!["bug".to_string(), "issue".to_string(), "task".to_string()],
        )]));

        let rules = lm.lookup_rules(&[node_created_key("bug")]);
        assert_eq!(
            rules.len(),
            1,
            "ancestry matching must span the whole chain, not one level"
        );
    }

    #[test]
    fn a_play_on_an_unrelated_type_does_not_fire() {
        let mut lm = manager_with_play_on("project");
        lm.set_ancestor_cache(HashMap::from([(
            "issue".to_string(),
            vec!["issue".to_string(), "task".to_string()],
        )]));

        let rules = lm.lookup_rules(&[node_created_key("issue")]);
        assert!(
            rules.is_empty(),
            "ancestry widens matching along the chain only, not across unrelated types"
        );
    }

    #[test]
    fn exact_type_plays_still_match_exactly_with_no_ancestry() {
        let lm = manager_with_play_on("task");

        // No ancestor cache at all — the state of every database until a
        // schema declares `extends`.
        assert_eq!(
            lm.lookup_rules(&[node_created_key("task")]).len(),
            1,
            "an exact match must still match"
        );
        assert!(
            lm.lookup_rules(&[node_created_key("issue")]).is_empty(),
            "with no ancestry, an unrelated type must not match"
        );
    }

    #[test]
    fn a_subtype_scoped_play_does_not_fire_on_its_base() {
        let mut lm = manager_with_play_on("issue");
        lm.set_ancestor_cache(HashMap::from([(
            "issue".to_string(),
            vec!["issue".to_string(), "task".to_string()],
        )]));

        // Matching runs child -> ancestor, never the reverse: an Issue is a
        // Task, but a Task is not an Issue.
        let rules = lm.lookup_rules(&[node_created_key("task")]);
        assert!(
            rules.is_empty(),
            "a Play registered on a subtype must not fire for its base type"
        );
    }

    #[test]
    fn a_matching_rule_is_returned_once_not_per_ancestor() {
        let mut lm = PlaybookLifecycleManager::new();
        // One play, two rules: one on the base, one on the concrete type.
        // Both match an `issue` event, and each must appear exactly once.
        let node = make_play_node(
            "pb-dup",
            json!([
                {
                    "name": "on_task",
                    "trigger": { "type": "graph_event", "on": "node_created", "node_type": "task" },
                    "conditions": [],
                    "actions": []
                },
                {
                    "name": "on_issue",
                    "trigger": { "type": "graph_event", "on": "node_created", "node_type": "issue" },
                    "conditions": [],
                    "actions": []
                }
            ]),
        );
        lm.activate_play(&node)
            .expect("play activation should succeed");
        lm.set_ancestor_cache(HashMap::from([(
            "issue".to_string(),
            vec!["issue".to_string(), "task".to_string()],
        )]));

        let rules = lm.lookup_rules(&[node_created_key("issue")]);
        assert_eq!(
            rules.len(),
            2,
            "both rules match once each; dedup must not collapse distinct rules, \
             and fan-out must not duplicate either"
        );
    }

    #[test]
    fn ancestors_of_falls_back_to_the_type_itself() {
        let lm = PlaybookLifecycleManager::new();
        assert_eq!(lm.ancestors_of("task"), vec!["task".to_string()]);
        assert!(!lm.has_ancestry());
    }

    #[test]
    fn property_changed_keys_fan_out_to_ancestors_too() {
        let mut lm = PlaybookLifecycleManager::new();
        let node = make_play_node(
            "pb-prop",
            json!([{
                "name": "r1",
                "trigger": {
                    "type": "graph_event",
                    "on": "property_changed",
                    "node_type": "task",
                    "property_key": "task.status"
                },
                "conditions": [],
                "actions": []
            }]),
        );
        lm.activate_play(&node)
            .expect("play activation should succeed");
        lm.set_ancestor_cache(HashMap::from([(
            "issue".to_string(),
            vec!["issue".to_string(), "task".to_string()],
        )]));

        // The property key must be carried across the re-key, not dropped —
        // and, per `renamespace_property_key`, re-namespaced from the event's
        // own type to the ancestor's. A real PropertyChanged event on an
        // `issue` node carries "issue.status" (never bare "status" — that
        // spelling is production-impossible and is now rejected at save time
        // by `validate_play`), so that is what a real fan-out lookup key
        // looks like.
        let rules = lm.lookup_rules(&[TriggerKey::NodeEvent {
            event: NodeEventType::PropertyChanged,
            node_type: "issue".to_string(),
            property_key: Some("issue.status".to_string()),
        }]);
        assert_eq!(
            rules.len(),
            1,
            "a property-scoped Play on the base should match the same property on a subtype"
        );
    }

    /// The same fan-out, with the TYPE-NAMESPACED key a real event carries.
    ///
    /// `PropertyChanged` events spell the key `<node_type>.<field>`
    /// (`PropertyChange::key`, "namespaced, e.g. task.status"), and a Play
    /// registered on `task` indexes itself under `task.status`. Fanning a
    /// subtype's event out by rewriting only `node_type` yields
    /// `(task, "bug.status")` — a key nothing is registered under — so the
    /// Play never fires on a subtype at all.
    ///
    /// The sibling test above uses a bare `status`, which has no namespace to
    /// move and so cannot catch this. That is why the bug survived: the
    /// mechanism was tested only in the one spelling where it could not fail.
    #[test]
    fn namespaced_property_keys_are_renamespaced_when_fanning_out_to_an_ancestor() {
        let mut lm = PlaybookLifecycleManager::new();
        let node = make_play_node(
            "pb-prop-ns",
            json!([{
                "name": "r1",
                "trigger": {
                    "type": "graph_event",
                    "on": "property_changed",
                    "node_type": "task",
                    "property_key": "task.status"
                },
                "conditions": [],
                "actions": []
            }]),
        );
        lm.activate_play(&node)
            .expect("play activation should succeed");
        lm.set_ancestor_cache(HashMap::from([(
            "bug".to_string(),
            vec!["bug".to_string(), "task".to_string()],
        )]));

        let rules = lm.lookup_rules(&[TriggerKey::NodeEvent {
            event: NodeEventType::PropertyChanged,
            node_type: "bug".to_string(),
            property_key: Some("bug.status".to_string()),
        }]);
        assert_eq!(
            rules.len(),
            1,
            "a subtype's namespaced property event must reach a base-scoped Play"
        );

        // A field the base does not declare still re-namespaces, and simply
        // matches nothing — the fan-out re-keys, it does not filter.
        let unmatched = lm.lookup_rules(&[TriggerKey::NodeEvent {
            event: NodeEventType::PropertyChanged,
            node_type: "bug".to_string(),
            property_key: Some("bug.severity".to_string()),
        }]);
        assert!(
            unmatched.is_empty(),
            "a subtype-only field must not match a Play registered on another field"
        );
    }

    #[test]
    fn renamespace_property_key_only_moves_a_matching_leading_namespace() {
        assert_eq!(
            renamespace_property_key("bug.status", "bug", "task"),
            "task.status"
        );
        // Bare keys have no namespace to move.
        assert_eq!(renamespace_property_key("status", "bug", "task"), "status");
        // A namespace belonging to some other type is left alone.
        assert_eq!(
            renamespace_property_key("other.status", "bug", "task"),
            "other.status"
        );
        // Only the leading segment is replaced; a dotted tail survives.
        assert_eq!(
            renamespace_property_key("bug.a.b", "bug", "task"),
            "task.a.b"
        );
    }
}
