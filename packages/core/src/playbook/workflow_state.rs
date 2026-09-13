//! `get-workflow-state` — out-of-band evaluation of active Play rules against
//! a single node.
//!
//! This is not a mechanical CLI wrapper around the engine's live-trigger path
//! (`engine.rs`/`cel.rs`). It answers a different question: "if this node's
//! current state were evaluated right now, which rules would fire?" — with no
//! real triggering `DomainEvent`, run out of band from any actual mutation.
//! Three design points, resolved here rather than left to accumulate as ad
//! hoc behavior:
//!
//! 1. **Fired-state scoping.** The engine tracks no persisted "this rule has
//!    fired for this node" record — logs record only failures, and per
//!    ADR-073 there is no cross-device fired-state at all. So this module
//!    reports live condition satisfaction (computed fresh, right now, on this
//!    device), never a historical "already fired" claim it cannot back up.
//! 2. **Synthetic trigger event.** There is no real mutation to build a
//!    `DomainEvent` from, so conditions are evaluated against a synthesized
//!    `NodeCreated`-shaped event. `trigger.property.old_value`/`new_value`
//!    bindings (meaningful only for a real `property_changed` firing) are
//!    left absent rather than fabricated — a condition that references them
//!    is reported as unresolvable, not silently given made-up values.
//! 3. **Evaluation scope.** Candidate rules are found via the engine's own
//!    `TriggerIndex` (`PlaybookLifecycleManager::lookup_rules`), keyed off
//!    the queried node's type, not a linear scan of every active play.

use crate::db::events::DomainEvent;
use crate::models::Node;
use crate::playbook::cel::{self, ConditionResult};
use crate::playbook::graph_resolver::GraphResolver;
use crate::playbook::lifecycle::PlaybookLifecycleManager;
use crate::playbook::path_extractor;
use crate::playbook::types::{NodeEventType, TriggerKey};
use crate::services::NodeService;
use serde::Serialize;
use std::sync::{Arc, RwLock};

/// One condition's evaluated state within a rule, for a `get-workflow-state` query.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum ConditionState {
    /// The condition evaluated to `true`.
    Satisfied,
    /// The condition evaluated to `false` because a referenced path traverses
    /// a real, schema-declared relationship or field that simply has no value
    /// yet — the spec's "not yet met, play stays active" case.
    NotYetMet { condition: String },
    /// The condition references something that does not exist on the node's
    /// schema at all (neither a field nor a declared relationship) — almost
    /// certainly a typo, and will never resolve no matter what the graph
    /// looks like. Distinct from `NotYetMet` so a hand-authoring agent isn't
    /// told to "wait" for something that can never happen.
    Unresolvable { condition: String, reason: String },
}

/// One rule's evaluated state within a `get-workflow-state` response.
#[derive(Debug, Clone, Serialize)]
pub struct RuleWorkflowState {
    pub play_id: String,
    pub rule_name: String,
    pub rule_index: usize,
    /// `true` only if every condition evaluated `Satisfied`.
    pub all_conditions_satisfied: bool,
    pub conditions: Vec<ConditionState>,
}

/// Full response for a `get-workflow-state` query against one node.
#[derive(Debug, Clone, Serialize)]
pub struct WorkflowState {
    pub node_id: String,
    pub node_type: String,
    /// States this response is scoped to — always exactly `["local"]` today.
    /// Present as a field (not just documented) so a caller that stores or
    /// forwards this response carries the scope with it, per ADR-073: no
    /// cross-device fired-state exists, and this field is the machine-
    /// readable form of that limitation rather than prose a caller can miss.
    pub scope: Vec<String>,
    /// Explicit note that "fired" history is not tracked anywhere and this
    /// response reports live condition state only, not execution history.
    pub fired_state_note: String,
    pub rules: Vec<RuleWorkflowState>,
}

/// Evaluate every active Play rule whose trigger matches `node`'s type against
/// `node`'s current state, and report per-condition satisfaction.
///
/// Candidate rules come from the engine's `TriggerIndex` via
/// `lookup_rules` (synthesized `NodeCreated` + wildcard `PropertyChanged` keys
/// for the node's type) — not a linear scan of every active play. Rules whose
/// trigger is `scheduled` rather than `graph_event` are included too: a
/// scheduled trigger's `node_type` scopes which nodes the engine scans, so
/// membership in this node's type is the same eligibility test.
pub async fn get_workflow_state(
    lifecycle: &Arc<RwLock<PlaybookLifecycleManager>>,
    node_service: &Arc<NodeService>,
    node: &Node,
) -> WorkflowState {
    let schema = node_service
        .get_schema_with_relationships(&node.node_type)
        .await
        .ok()
        .flatten();

    let candidate_refs = {
        let lm = lifecycle.read().expect("lifecycle lock poisoned");
        let mut keys = vec![
            TriggerKey::NodeEvent {
                event: NodeEventType::NodeCreated,
                node_type: node.node_type.clone(),
                property_key: None,
            },
            // The wildcard PropertyChanged key: matches a rule registered
            // with no specific property_key (fires on any property change).
            TriggerKey::NodeEvent {
                event: NodeEventType::PropertyChanged,
                node_type: node.node_type.clone(),
                property_key: None,
            },
        ];

        // A rule registered for a SPECIFIC property_key is indexed under
        // that exact key (see lifecycle::trigger_keys_for_graph_event), not
        // the wildcard — and there is no real changed-property list here to
        // derive that key from (this is an out-of-band query, not a live
        // mutation). The schema's declared field names are the bounded,
        // known-in-advance set of property keys a rule on this node_type
        // could plausibly be registered under, so one exact-key lookup per
        // declared field covers every such rule without linearly scanning
        // every active play.
        if let Some(s) = &schema {
            for field in &s.fields {
                keys.push(TriggerKey::NodeEvent {
                    event: NodeEventType::PropertyChanged,
                    node_type: node.node_type.clone(),
                    property_key: Some(field.name.clone()),
                });
            }
        }

        let mut refs = lm.lookup_rules(&keys);

        // Scheduled rules aren't in the graph-event TriggerIndex at all —
        // they live in the CronRegistry, keyed by node_type. Include them
        // here too: "what would fire for this node" should cover a
        // scheduled rule the same way it covers a graph-event one, since
        // both are just "conditions evaluated against this node's state."
        for entry in lm.cron_registry() {
            if entry.node_type == node.node_type {
                for r in &entry.rules {
                    if !refs.iter().any(|existing| existing == r) {
                        refs.push(r.clone());
                    }
                }
            }
        }
        refs
    };

    // Synthetic trigger event: no real mutation occurred, so this is shaped
    // as a NodeCreated event. `trigger.property.*` bindings are therefore
    // absent from the CEL context (see cel::build_condition_context) — a
    // condition that reads them will surface as Unresolvable below, not be
    // given a fabricated old/new value.
    let synthetic_event = DomainEvent::NodeCreated {
        node_type: node.node_type.clone(),
        node_id: node.id.clone(),
    };

    let mut rules = Vec::with_capacity(candidate_refs.len());
    for rule_ref in &candidate_refs {
        let mut resolver = GraphResolver::new(Arc::clone(node_service));
        let mut condition_states = Vec::with_capacity(rule_ref.rule.conditions.len());

        for condition in &rule_ref.rule.conditions {
            let state = evaluate_one_condition(
                condition,
                node,
                &synthetic_event,
                &mut resolver,
                schema.as_ref(),
            )
            .await;
            condition_states.push(state);
        }

        let all_satisfied = condition_states
            .iter()
            .all(|c| matches!(c, ConditionState::Satisfied));

        rules.push(RuleWorkflowState {
            play_id: rule_ref.play_id.clone(),
            rule_name: rule_ref.rule.name.clone(),
            rule_index: rule_ref.rule_index,
            all_conditions_satisfied: all_satisfied,
            conditions: condition_states,
        });
    }

    WorkflowState {
        node_id: node.id.clone(),
        node_type: node.node_type.clone(),
        scope: vec!["local".to_string()],
        fired_state_note: "This reports live condition satisfaction computed just now, on this \
             device. Whether a rule has previously fired is not tracked anywhere in the system \
             (per ADR-073, there is no cross-device fired-state yet) — this is not an execution \
             history."
            .to_string(),
        rules,
    }
}

/// Evaluate a single condition and classify its result as satisfied, not-yet-met,
/// or unresolvable.
///
/// Reuses `cel::evaluate_conditions` (a one-condition slice) for the actual
/// evaluation so this can never silently diverge from live-trigger semantics
/// — the classification layer added here is purely about *why* a `false`
/// happened, not a second evaluation path.
async fn evaluate_one_condition(
    condition: &cel::CompiledCondition,
    node: &Node,
    event: &DomainEvent,
    resolver: &mut GraphResolver,
    schema: Option<&crate::models::SchemaNode>,
) -> ConditionState {
    let result =
        cel::evaluate_conditions(std::slice::from_ref(condition), node, event, Some(resolver))
            .await;

    match result {
        ConditionResult::Pass => ConditionState::Satisfied,
        ConditionResult::Fail { .. } => {
            classify_failure(condition, node, schema).unwrap_or(ConditionState::NotYetMet {
                condition: condition.source.clone(),
            })
        }
    }
}

/// Distinguish a legitimately-unmet condition from one that can never resolve.
///
/// Extracts every dot-path the condition references and checks its first
/// segment (after the `node`/`trigger` root) against the node's schema: a
/// name that is neither a declared field nor a declared relationship cannot
/// possibly resolve later, however the graph evolves, and is reported as
/// `Unresolvable`. A name that *is* a declared relationship with (for now) no
/// target is exactly the spec's "not yet met" case.
///
/// Returns `None` when extraction finds nothing conclusive, so the caller
/// falls back to the conservative `NotYetMet` classification.
fn classify_failure(
    condition: &cel::CompiledCondition,
    node: &Node,
    schema: Option<&crate::models::SchemaNode>,
) -> Option<ConditionState> {
    let extraction = path_extractor::extract_paths(&condition.source).ok()?;

    let known_fields: Vec<&str> = schema
        .map(|s| s.fields.iter().map(|f| f.name.as_str()).collect())
        .unwrap_or_default();
    let known_relationships: Vec<&str> = schema
        .map(|s| s.relationships.iter().map(|r| r.name.as_str()).collect())
        .unwrap_or_default();

    // Core node fields resolvable with no schema lookup at all — a condition
    // naming one of these is never a typo regardless of what the schema declares.
    const CORE_FIELDS: &[&str] = &["id", "node_type", "content", "version", "lifecycle_status"];

    for path in &extraction.paths {
        // Only "node.<segment>..." paths name something on this node's own
        // schema; "trigger.property.*" is a different root entirely (see the
        // module doc — those bindings are intentionally absent here) and is
        // handled by the trigger-property check below instead.
        if path.root == "trigger" {
            if path.segments.get(1).map(String::as_str) == Some("property") {
                return Some(ConditionState::Unresolvable {
                    condition: condition.source.clone(),
                    reason: "this condition reads trigger.property.old_value/new_value, which \
                             only exist during a real property_changed firing — get-workflow-state \
                             evaluates out of band, with no such event, so this can never resolve here"
                        .to_string(),
                });
            }
            continue;
        }
        if path.root != "node" || path.segments.len() < 2 {
            continue;
        }

        let first_segment = path.segments[1].as_str();
        let is_known = CORE_FIELDS.contains(&first_segment)
            || known_fields.contains(&first_segment)
            || known_relationships.contains(&first_segment);

        if !is_known {
            return Some(ConditionState::Unresolvable {
                condition: condition.source.clone(),
                reason: format!(
                    "'{first_segment}' is not a declared field or relationship on schema '{}' \
                     — likely a typo, since no future graph state can make this resolve",
                    node.node_type
                ),
            });
        }

        // A declared relationship with more segments after it (multi-hop) or
        // alone (single-hop, no target yet) that failed to resolve is exactly
        // the "not yet met" case — the name is real, the edge just doesn't
        // exist yet.
        if known_relationships.contains(&first_segment) {
            return Some(ConditionState::NotYetMet {
                condition: condition.source.clone(),
            });
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::playbook::lifecycle::PlaybookLifecycleManager;
    use chrono::Utc;
    use serde_json::json;

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

    fn make_test_node(node_type: &str, properties: serde_json::Value) -> Node {
        Node {
            id: "test-node-1".to_string(),
            node_type: node_type.to_string(),
            content: "Test content".to_string(),
            version: 1,
            created_at: Utc::now(),
            modified_at: Utc::now(),
            properties,
            mentions: vec![],
            mentioned_in: vec![],
            title: None,
            lifecycle_status: "active".to_string(),
        }
    }

    async fn test_service() -> (Arc<NodeService>, tempfile::TempDir) {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let db_path = temp_dir.path().join("test.db");
        let mut store: Arc<crate::db::SqliteStore> =
            Arc::new(crate::db::SqliteStore::new(db_path).await.unwrap());
        let node_service = Arc::new(NodeService::new(&mut store).await.unwrap());
        (node_service, temp_dir)
    }

    #[tokio::test]
    async fn satisfied_condition_reports_satisfied() {
        let (svc, _tmp) = test_service().await;
        let lifecycle = Arc::new(RwLock::new(PlaybookLifecycleManager::new()));
        {
            let mut lm = lifecycle.write().unwrap();
            let play = make_play_node(
                "pb-1",
                json!([{
                    "name": "r1",
                    "trigger": { "type": "graph_event", "on": "node_created", "node_type": "task" },
                    "conditions": ["node.status == 'open'"],
                    "actions": []
                }]),
            );
            lm.activate_play(&play).unwrap();
        }

        let node = make_test_node("task", json!({"status": "open"}));
        let state = get_workflow_state(&lifecycle, &svc, &node).await;

        assert_eq!(state.scope, vec!["local".to_string()]);
        assert_eq!(state.rules.len(), 1);
        assert!(state.rules[0].all_conditions_satisfied);
        assert_eq!(state.rules[0].conditions[0], ConditionState::Satisfied);
    }

    #[tokio::test]
    async fn unmet_relationship_reports_not_yet_met() {
        let (svc, _tmp) = test_service().await;

        let schema = Node::new_with_id(
            "story".to_string(),
            "schema".to_string(),
            "story".to_string(),
            json!({
                "isCore": false, "schemaVersion": 1, "description": "story",
                "fields": [],
                "relationships": []
            }),
        );
        svc.create_node(schema).await.unwrap();

        let task_schema = Node::new_with_id(
            "wf_task".to_string(),
            "schema".to_string(),
            "wf_task".to_string(),
            json!({
                "isCore": false, "schemaVersion": 1, "description": "wf_task",
                "fields": [{"name": "status", "friendlyName": "Status", "type": "string"}],
                "relationships": []
            }),
        );
        svc.create_node(task_schema).await.unwrap();
        svc.set_schema_relationships(
            "wf_task",
            &[serde_json::from_value(json!({
                "name": "story",
                "targetType": "story",
                "direction": "out",
                "cardinality": "one",
                "reverseName": "tasks",
                "reverseCardinality": "many"
            }))
            .unwrap()],
        )
        .await
        .unwrap();

        let lifecycle = Arc::new(RwLock::new(PlaybookLifecycleManager::new()));
        {
            let mut lm = lifecycle.write().unwrap();
            let play = make_play_node(
                "pb-2",
                json!([{
                    "name": "r1",
                    "trigger": { "type": "graph_event", "on": "node_created", "node_type": "wf_task" },
                    "conditions": ["node.story.status == 'active'"],
                    "actions": []
                }]),
            );
            lm.activate_play(&play).unwrap();
        }

        let task = make_test_node("wf_task", json!({"status": "open"}));
        svc.create_node(task.clone()).await.unwrap();

        let state = get_workflow_state(&lifecycle, &svc, &task).await;
        assert_eq!(state.rules.len(), 1);
        assert!(!state.rules[0].all_conditions_satisfied);
        match &state.rules[0].conditions[0] {
            ConditionState::NotYetMet { condition } => {
                assert_eq!(condition, "node.story.status == 'active'");
            }
            other => panic!("expected NotYetMet, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn typo_field_reports_unresolvable() {
        let (svc, _tmp) = test_service().await;

        let task_schema = Node::new_with_id(
            "wf_task2".to_string(),
            "schema".to_string(),
            "wf_task2".to_string(),
            json!({
                "isCore": false, "schemaVersion": 1, "description": "wf_task2",
                "fields": [{"name": "status", "friendlyName": "Status", "type": "string"}],
                "relationships": []
            }),
        );
        svc.create_node(task_schema).await.unwrap();

        let lifecycle = Arc::new(RwLock::new(PlaybookLifecycleManager::new()));
        {
            let mut lm = lifecycle.write().unwrap();
            let play = make_play_node(
                "pb-3",
                json!([{
                    "name": "r1",
                    "trigger": { "type": "graph_event", "on": "node_created", "node_type": "wf_task2" },
                    "conditions": ["node.staatus == 'open'"],
                    "actions": []
                }]),
            );
            lm.activate_play(&play).unwrap();
        }

        let task = make_test_node("wf_task2", json!({"status": "open"}));
        svc.create_node(task.clone()).await.unwrap();

        let state = get_workflow_state(&lifecycle, &svc, &task).await;
        assert_eq!(state.rules.len(), 1);
        match &state.rules[0].conditions[0] {
            ConditionState::Unresolvable { reason, .. } => {
                assert!(reason.contains("staatus"), "reason was: {reason}");
                assert!(reason.contains("typo"), "reason was: {reason}");
            }
            other => panic!("expected Unresolvable, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn trigger_property_reference_reports_unresolvable() {
        let (svc, _tmp) = test_service().await;

        let task_schema = Node::new_with_id(
            "wf_task3".to_string(),
            "schema".to_string(),
            "wf_task3".to_string(),
            json!({
                "isCore": false, "schemaVersion": 1, "description": "wf_task3",
                "fields": [{"name": "status", "friendlyName": "Status", "type": "string"}],
                "relationships": []
            }),
        );
        svc.create_node(task_schema).await.unwrap();

        let lifecycle = Arc::new(RwLock::new(PlaybookLifecycleManager::new()));
        {
            let mut lm = lifecycle.write().unwrap();
            let play = make_play_node(
                "pb-4",
                json!([{
                    "name": "r1",
                    "trigger": { "type": "graph_event", "on": "property_changed", "node_type": "wf_task3", "property_key": "status" },
                    "conditions": ["trigger.property.old_value == 'open'"],
                    "actions": []
                }]),
            );
            lm.activate_play(&play).unwrap();
        }

        let task = make_test_node("wf_task3", json!({"status": "done"}));
        let state = get_workflow_state(&lifecycle, &svc, &task).await;
        assert_eq!(state.rules.len(), 1);
        match &state.rules[0].conditions[0] {
            ConditionState::Unresolvable { reason, .. } => {
                assert!(reason.contains("property_changed"), "reason was: {reason}");
            }
            other => panic!("expected Unresolvable, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn no_matching_rules_returns_empty() {
        let (svc, _tmp) = test_service().await;
        let lifecycle = Arc::new(RwLock::new(PlaybookLifecycleManager::new()));
        let node = make_test_node("invoice", json!({}));
        let state = get_workflow_state(&lifecycle, &svc, &node).await;
        assert!(state.rules.is_empty());
    }

    #[tokio::test]
    async fn scheduled_trigger_rules_are_included() {
        let (svc, _tmp) = test_service().await;
        let lifecycle = Arc::new(RwLock::new(PlaybookLifecycleManager::new()));
        {
            let mut lm = lifecycle.write().unwrap();
            let play = make_play_node(
                "pb-5",
                json!([{
                    "name": "r1",
                    "trigger": { "type": "scheduled", "cron": "0 9 * * *", "node_type": "invoice" },
                    "conditions": ["node.status == 'overdue'"],
                    "actions": []
                }]),
            );
            lm.activate_play(&play).unwrap();
        }

        let node = make_test_node("invoice", json!({"status": "overdue"}));
        let state = get_workflow_state(&lifecycle, &svc, &node).await;
        assert_eq!(state.rules.len(), 1);
        assert!(state.rules[0].all_conditions_satisfied);
    }
}
