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
use crate::playbook::types::{namespaced_property_key, NodeEventType, TriggerKey};
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
    /// Non-empty when a schema/extends-chain resolution call
    /// (`resolve_field_owners`/`resolve_relationships`/a schema fetch)
    /// failed while building this response and was degraded to a narrower,
    /// less complete field or relationship set rather than aborting the
    /// whole query. Each entry names what failed and where.
    ///
    /// Consistent with this module's "never silently given a made-up value"
    /// policy (see the module doc): a transient DB error during this merge
    /// must not be able to silently reintroduce the exact under-reporting /
    /// typo-misclassification bug this diagnostic exists to avoid. An empty
    /// vec means every lookup that fed this response succeeded; a non-empty
    /// one means this response may under-report candidates or misclassify a
    /// condition as `Unresolvable` that a successful lookup would have
    /// correctly classified as `NotYetMet` or `Satisfied` — a caller should
    /// treat the response as incomplete, not authoritative, until a retry
    /// comes back with an empty `degraded_reasons`.
    pub degraded_reasons: Vec<String>,
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
    // Collects a human-readable entry every time a schema/extends-chain
    // lookup fails and this function degrades to a narrower field or
    // relationship set instead of aborting — see `WorkflowState::degraded_reasons`.
    // Threaded through `evaluate_one_condition`/`classify_failure`/
    // `walk_path_against_schema` too, so a per-hop failure during condition
    // classification is visible on the final response the same way a
    // top-level failure here is.
    let mut degraded: Vec<String> = Vec::new();

    let schema = match node_service
        .get_schema_with_relationships(&node.node_type)
        .await
    {
        Ok(schema) => schema,
        Err(e) => {
            // A real lookup failure (not "no schema for this type") — the
            // typo-vs-unmet distinction degrades to the conservative
            // NotYetMet classification for this node, but that degradation
            // should be visible rather than indistinguishable from a genuine
            // schemaless type.
            let msg = format!(
                "schema lookup for '{}' failed ({e}); typo detection degraded to NotYetMet for this node",
                node.node_type
            );
            tracing::warn!(node_type = %node.node_type, error = %e, "get_workflow_state: {msg}");
            degraded.push(msg);
            None
        }
    };

    // The candidate-key enumeration needs the *effective* field set across
    // `node.node_type`'s extends chain (ADR-078) — not just `schema`'s own
    // directly-declared fields. A subtype node whose triggering field is
    // only declared on an ancestor schema (the normal, intended `extends`
    // usage — not redeclaring inherited fields) must still get a candidate
    // key built for it, or a genuinely active, satisfied rule silently never
    // shows up here. `resolve_field_owners` already walks that chain and
    // merges it; reuse it rather than re-deriving the merge from `schema`.
    let effective_fields = match node_service.resolve_field_owners(&node.node_type).await {
        Ok((fields, _owners, _chain)) => fields,
        Err(e) => {
            let msg = format!(
                "effective-field resolution for '{}' failed ({e}); property_changed \
                 candidates degraded to this node's own directly-declared schema fields",
                node.node_type
            );
            tracing::warn!(node_type = %node.node_type, error = %e, "get_workflow_state: {msg}");
            degraded.push(msg);
            schema
                .as_ref()
                .map(|s| s.fields.clone())
                .unwrap_or_default()
        }
    };

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
        // mutation). The effective field set (own schema + everything
        // inherited across the extends chain) is the bounded, known-in-
        // advance set of property keys a rule on this node_type could
        // plausibly be registered under, so one exact-key lookup per field
        // covers every such rule without linearly scanning every active
        // play. The trigger key itself is always type-namespaced
        // (`<node_type>.<field>`, e.g. "task.status" — see
        // `validate_play`'s `UnnamespacedPropertyChangedKey` check and
        // `trigger_keys_for_graph_event`, which indexes verbatim under
        // whatever `property_key` a rule declared), so the lookup key built
        // here must match that same namespaced shape or it can never hit.
        for field in &effective_fields {
            keys.push(TriggerKey::NodeEvent {
                event: NodeEventType::PropertyChanged,
                node_type: node.node_type.clone(),
                property_key: Some(namespaced_property_key(&node.node_type, &field.name)),
            });
        }

        let mut refs = lm.lookup_rules(&keys);

        // Scheduled rules aren't in the graph-event TriggerIndex at all —
        // they live in the CronRegistry, keyed by node_type. Include them
        // here too: "what would fire for this node" should cover a
        // scheduled rule the same way it covers a graph-event one, since
        // both are just "conditions evaluated against this node's state."
        //
        // `lookup_rules` above fans a graph-event candidate out across
        // `node.node_type`'s full ADR-078 ancestry for free (via
        // `PlaybookLifecycleManager::ancestor_keys`) — an exact-string
        // `node_type` match here would be inconsistent with that and would
        // silently drop a scheduled Play registered on a base type from
        // this response for every subtype node. `ancestors_of` returns the
        // same ancestry `lookup_rules` uses internally (nearest first,
        // including the type itself), so membership in it is the matching
        // eligibility test for a scheduled trigger too.
        let ancestry = lm.ancestors_of(&node.node_type);
        for entry in lm.cron_registry() {
            if ancestry.iter().any(|t| t == &entry.node_type) {
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
    // One resolver across all rules for this node: every rule resolves paths
    // from the same root, so rebuilding it per rule threw away a cache that was
    // about to be asked the same questions. Safe because cache entries are
    // scoped to the root they were resolved from.
    let mut resolver = GraphResolver::new(Arc::clone(node_service));
    for rule_ref in &candidate_refs {
        let mut condition_states = Vec::with_capacity(rule_ref.rule.conditions.len());

        for condition in &rule_ref.rule.conditions {
            let state = evaluate_one_condition(
                condition,
                node,
                &synthetic_event,
                &mut resolver,
                node_service,
                schema.as_ref(),
                &mut degraded,
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
        degraded_reasons: degraded,
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
    node_service: &Arc<NodeService>,
    schema: Option<&crate::models::SchemaNode>,
    degraded: &mut Vec<String>,
) -> ConditionState {
    let result =
        cel::evaluate_conditions(std::slice::from_ref(condition), node, event, Some(resolver))
            .await;

    match result {
        ConditionResult::Pass => ConditionState::Satisfied,
        ConditionResult::Fail { .. } => {
            match classify_failure(condition, node, node_service, schema, degraded).await {
                Some(state) => state,
                None => ConditionState::NotYetMet {
                    condition: condition.source.clone(),
                },
            }
        }
    }
}

/// Core node fields resolvable with no schema lookup at all — a condition
/// naming one of these is never a typo regardless of what the schema declares.
const CORE_FIELDS: &[&str] = &["id", "node_type", "content", "version", "lifecycle_status"];

/// Distinguish a legitimately-unmet condition from one that can never resolve.
///
/// Extracts every dot-path the condition references and walks each one
/// hop-by-hop against the schema chain it traverses: `node.story.epic.status`
/// checks `story` against `node`'s own schema, then (if `story` is a real
/// relationship) follows its `target_type` to fetch *that* schema and checks
/// `epic` against it, and so on. A segment that names neither a declared
/// field nor a declared relationship on the schema reached at that point in
/// the walk cannot possibly resolve later, however the graph evolves, and is
/// reported as `Unresolvable`. A walk that runs out of segments while every
/// hop so far was a real, declared relationship is exactly the spec's "not
/// yet met" case — the path is legitimate, the edge just doesn't exist yet.
///
/// Returns `None` when extraction finds nothing conclusive, so the caller
/// falls back to the conservative `NotYetMet` classification.
async fn classify_failure(
    condition: &cel::CompiledCondition,
    node: &Node,
    node_service: &Arc<NodeService>,
    schema: Option<&crate::models::SchemaNode>,
    degraded: &mut Vec<String>,
) -> Option<ConditionState> {
    let extraction = path_extractor::extract_paths(&condition.source).ok()?;

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

        if let Some(state) = walk_path_against_schema(
            condition,
            node,
            node_service,
            schema,
            &path.segments[1..],
            degraded,
        )
        .await
        {
            return Some(state);
        }
    }

    None
}

/// Walk one dot-path's segments against the schema chain it traverses,
/// starting from `first_schema` (the node's own schema). Returns
/// `Some(Unresolvable)` as soon as a segment names neither a field nor a
/// relationship on the schema reached so far; returns `Some(NotYetMet)` if
/// every segment up to the last was a real, declared relationship (the path
/// is legitimate, just not populated yet); returns `None` if a schema lookup
/// along the way fails or a segment is a plain field (nothing further to
/// check — the path is fully explained without needing a verdict here).
async fn walk_path_against_schema(
    condition: &cel::CompiledCondition,
    node: &Node,
    node_service: &Arc<NodeService>,
    first_schema: Option<&crate::models::SchemaNode>,
    segments: &[String],
    degraded: &mut Vec<String>,
) -> Option<ConditionState> {
    let mut current_schema_owned: Option<crate::models::SchemaNode> = first_schema.cloned();
    let mut current_type = node.node_type.clone();

    for (i, segment) in segments.iter().enumerate() {
        let current_schema = current_schema_owned.as_ref();

        // The effective/merged field set across `current_type`'s extends
        // chain (ADR-078) — not just `current_schema`'s own directly-
        // declared fields. A subtype schema that inherits a field from an
        // ancestor without redeclaring it (the normal, intended usage) must
        // still count as a real field here, or a condition referencing it
        // is misclassified as Unresolvable ("likely a typo") instead of the
        // correct NotYetMet.
        let known_fields: Vec<String> = match node_service.resolve_field_owners(&current_type).await
        {
            Ok((fields, _owners, _chain)) => fields.into_iter().map(|f| f.name).collect(),
            Err(e) => {
                // Degrade to `current_schema`'s own directly-declared fields
                // (the pre-fix behavior), not an empty set: before this
                // change, `known_fields` was a free in-memory read off
                // `current_schema` that could never independently fail. An
                // empty fallback here would make a real DB error (lock
                // contention, etc.) misreport a field that unambiguously
                // exists on the node's own schema as `Unresolvable` — worse
                // than pre-fix behavior, and inconsistent with
                // `get_workflow_state`'s matching fallback above. Recorded in
                // `degraded` (not just logged) so this can never silently
                // reproduce, under a transient DB error, the exact
                // under-reporting/misclassification bug this fix closes.
                let msg = format!(
                    "effective-field resolution for '{current_type}' failed ({e}) while walking \
                     '{}'; typo detection degraded to this schema's own directly-declared \
                     fields at this hop",
                    condition.source
                );
                tracing::warn!(node_type = %current_type, error = %e, "walk_path_against_schema: {msg}");
                degraded.push(msg);
                current_schema
                    .map(|s| s.fields.iter().map(|f| f.name.clone()).collect())
                    .unwrap_or_default()
            }
        };

        // Same extends-chain merge as `known_fields` above, but for declared
        // relationships: a subtype schema that inherits (doesn't redeclare) a
        // relationship from an ancestor must still be recognized here, or a
        // condition traversing it is misclassified as a typo the same way an
        // inherited field was before this fix.
        let relationship: Option<crate::models::schema::SchemaRelationship> = match node_service
            .resolve_relationships(&current_type)
            .await
        {
            Ok(rels) => rels.into_iter().find(|r| r.name == *segment),
            Err(e) => {
                let msg = format!(
                    "effective-relationship resolution for '{current_type}' failed ({e}) \
                         while walking '{}'; typo detection degraded to this schema's own \
                         directly-declared relationships at this hop",
                    condition.source
                );
                tracing::warn!(node_type = %current_type, error = %e, "walk_path_against_schema: {msg}");
                degraded.push(msg);
                current_schema
                    .and_then(|s| s.relationships.iter().find(|r| r.name == *segment))
                    .cloned()
            }
        };

        let is_field =
            CORE_FIELDS.contains(&segment.as_str()) || known_fields.iter().any(|f| f == segment);

        if let Some(rel) = relationship {
            // A declared relationship. If more segments follow, keep walking
            // into its target schema; if this is the last segment, the path
            // is legitimate and simply has no target yet.
            if i == segments.len() - 1 {
                return Some(ConditionState::NotYetMet {
                    condition: condition.source.clone(),
                });
            }
            let Some(target_type) = rel.target_type.clone() else {
                // A relationship with no fixed target_type (accepts any node
                // type) has no further schema to check against — nothing
                // conclusive to say, so stop here rather than guess.
                return None;
            };
            current_schema_owned = match node_service
                .get_schema_with_relationships(&target_type)
                .await
            {
                Ok(s) => s,
                Err(e) => {
                    // A genuine lookup failure, not "no schema for this
                    // type" (that's `Ok(None)`, left as-is below). Recorded
                    // rather than silently swallowed: `known_fields`/
                    // `relationship` at the *next* hop fall back to this
                    // schema's own fields/relationships, so losing it here
                    // to a transient error degrades the next hop's typo
                    // detection the same way the two fallbacks above do.
                    let msg = format!(
                        "schema lookup for '{target_type}' failed ({e}) while walking '{}'; \
                         typo detection for the remaining hops degraded to no known schema",
                        condition.source
                    );
                    tracing::warn!(node_type = %target_type, error = %e, "walk_path_against_schema: {msg}");
                    degraded.push(msg);
                    None
                }
            };
            current_type = target_type;
            continue;
        }

        if is_field {
            // A real field reached mid-path with more segments after it, or
            // as the terminal segment — either way this segment is fully
            // explained by the schema; nothing conclusive to add.
            return None;
        }

        // Neither a declared field nor a declared relationship at this hop.
        return Some(ConditionState::Unresolvable {
            condition: condition.source.clone(),
            reason: format!(
                "'{segment}' is not a declared field or relationship on schema '{current_type}' \
                 — likely a typo, since no future graph state can make this resolve"
            ),
        });
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
        assert!(
            state.degraded_reasons.is_empty(),
            "no schema/extends-chain lookup should fail on this happy path: {:?}",
            state.degraded_reasons
        );
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
                "fields": [{"name": "status", "friendlyName": "Status", "type": "string"}],
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

    /// Regression for the multi-hop typo gap: a second-hop segment that is
    /// neither a declared field nor a declared relationship on the schema
    /// reached at that hop must report Unresolvable, not fall through to
    /// NotYetMet just because the first hop (`story`) was a real relationship.
    #[tokio::test]
    async fn multi_hop_typo_at_second_segment_reports_unresolvable() {
        let (svc, _tmp) = test_service().await;

        let story_schema = Node::new_with_id(
            "story_mh".to_string(),
            "schema".to_string(),
            "story_mh".to_string(),
            json!({
                "isCore": false, "schemaVersion": 1, "description": "story_mh",
                "fields": [{"name": "status", "friendlyName": "Status", "type": "string"}],
                "relationships": []
            }),
        );
        svc.create_node(story_schema).await.unwrap();

        let task_schema = Node::new_with_id(
            "wf_task_mh".to_string(),
            "schema".to_string(),
            "wf_task_mh".to_string(),
            json!({
                "isCore": false, "schemaVersion": 1, "description": "wf_task_mh",
                "fields": [],
                "relationships": []
            }),
        );
        svc.create_node(task_schema).await.unwrap();
        svc.set_schema_relationships(
            "wf_task_mh",
            &[serde_json::from_value(json!({
                "name": "story",
                "targetType": "story_mh",
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
            // "story" is a real relationship on wf_task_mh, but "epic" is
            // neither a field nor a relationship on story_mh — a typo one
            // hop deeper than the single-hop case.
            let play = make_play_node(
                "pb-mh",
                json!([{
                    "name": "r1",
                    "trigger": { "type": "graph_event", "on": "node_created", "node_type": "wf_task_mh" },
                    "conditions": ["node.story.epic == 'active'"],
                    "actions": []
                }]),
            );
            lm.activate_play(&play).unwrap();
        }

        let task = make_test_node("wf_task_mh", json!({}));
        svc.create_node(task.clone()).await.unwrap();

        let state = get_workflow_state(&lifecycle, &svc, &task).await;
        assert_eq!(state.rules.len(), 1);
        match &state.rules[0].conditions[0] {
            ConditionState::Unresolvable { reason, .. } => {
                assert!(reason.contains("epic"), "reason was: {reason}");
                assert!(reason.contains("story_mh"), "reason was: {reason}");
            }
            other => panic!("expected Unresolvable, got {:?}", other),
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
                    "trigger": { "type": "graph_event", "on": "property_changed", "node_type": "wf_task3", "property_key": "wf_task3.status" },
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

    /// Regression: a `property_changed` trigger's `property_key` is always
    /// stored type-namespaced (`<node_type>.<field>`, e.g. "task.status" —
    /// the only spelling `validate_play` accepts, per
    /// `UnnamespacedPropertyChangedKey`), which is also the exact key
    /// `trigger_keys_for_graph_event` indexes it under. The candidate lookup
    /// here must build that same namespaced shape or a property-key-scoped
    /// rule can never be found, even when its condition is currently
    /// satisfied. Before the fix, this failed with `state.rules.len() == 0`.
    #[tokio::test]
    async fn namespaced_property_changed_trigger_is_returned_as_candidate() {
        let (svc, _tmp) = test_service().await;

        let task_schema = Node::new_with_id(
            "wf_task4".to_string(),
            "schema".to_string(),
            "wf_task4".to_string(),
            json!({
                "isCore": false, "schemaVersion": 1, "description": "wf_task4",
                "fields": [{"name": "status", "friendlyName": "Status", "type": "string"}],
                "relationships": []
            }),
        );
        svc.create_node(task_schema).await.unwrap();

        let lifecycle = Arc::new(RwLock::new(PlaybookLifecycleManager::new()));
        {
            let mut lm = lifecycle.write().unwrap();
            let play = make_play_node(
                "pb-6",
                json!([{
                    "name": "r1",
                    "trigger": { "type": "graph_event", "on": "property_changed", "node_type": "wf_task4", "property_key": "wf_task4.status" },
                    "conditions": ["node.status == 'done'"],
                    "actions": []
                }]),
            );
            lm.activate_play(&play).unwrap();
        }

        // Realistic on-disk storage shape: `NodeService::create_node` (via
        // `normalize_flat_properties_to_namespace`) always wraps a flat
        // property map in its owning type's namespace before persisting, so
        // a bare `{"status": "done"}` is not what a node created through the
        // real write path ever looks like on disk. Using the nested shape
        // here means this test would actually catch a future narrowing of
        // `GraphResolver`/`node_to_cel_value_at_scope`'s flat-shape fallback,
        // which is the only reason the flat shape passed before.
        let task = make_test_node("wf_task4", json!({"wf_task4": {"status": "done"}}));
        let state = get_workflow_state(&lifecycle, &svc, &task).await;
        assert_eq!(
            state.rules.len(),
            1,
            "expected the property-key-scoped rule to be returned as a candidate"
        );
        assert!(state.rules[0].all_conditions_satisfied);
        assert_eq!(state.rules[0].conditions[0], ConditionState::Satisfied);
    }

    /// Regression for the ADR-078 extends-chain gap: a subtype node whose
    /// triggering field is declared only on an ancestor schema (inherited,
    /// not redeclared — the normal, intended `extends` usage) must still get
    /// a `property_changed` candidate key built for it. Before the fix,
    /// `get_workflow_state` enumerated candidate keys from `schema.fields`
    /// alone — the subtype's own directly-declared fields, empty here — and
    /// never built the `wf_sub_pc.status` key at all, so this rule was
    /// silently absent from the response (`state.rules.len() == 0`) with no
    /// indication anything was skipped, even though it is genuinely active
    /// and satisfied.
    ///
    /// The condition deliberately reads a core field (`node.id`), not
    /// `status` itself: this isolates the candidate-key-enumeration fix
    /// under test from `get_workflow_state`'s separate, pre-existing,
    /// out-of-scope behavior of evaluating its synthetic event only against
    /// the node's own type-namespace bucket (see the module doc's "Synthetic
    /// trigger event" point) — a real inherited field's stored value lives in
    /// its owning ancestor's bucket, not the subtype's.
    #[tokio::test]
    async fn inherited_property_changed_trigger_is_returned_as_candidate() {
        let (svc, _tmp) = test_service().await;

        crate::schema::handle_create_schema(
            &svc,
            json!({
                "name": "wf_base_pc",
                "fields": [
                    { "name": "status", "type": "string", "protection": "user", "indexed": false }
                ]
            }),
        )
        .await
        .expect("base schema creation failed");

        crate::schema::handle_create_schema(
            &svc,
            json!({
                "name": "wf_sub_pc",
                "extends": "wf_base_pc",
                "fields": []
            }),
        )
        .await
        .expect("subtype schema creation failed");

        let lifecycle = Arc::new(RwLock::new(PlaybookLifecycleManager::new()));
        {
            let mut lm = lifecycle.write().unwrap();
            let play = make_play_node(
                "pb-inherit-pc",
                json!([{
                    "name": "r1",
                    "trigger": { "type": "graph_event", "on": "property_changed", "node_type": "wf_sub_pc", "property_key": "wf_sub_pc.status" },
                    "conditions": ["node.id != ''"],
                    "actions": []
                }]),
            );
            lm.activate_play(&play).unwrap();
        }

        let task = make_test_node("wf_sub_pc", json!({}));
        let state = get_workflow_state(&lifecycle, &svc, &task).await;
        assert_eq!(
            state.rules.len(),
            1,
            "expected the inherited-field property-key-scoped rule to be returned as a \
             candidate — before the fix this was silently 0"
        );
        assert!(state.rules[0].all_conditions_satisfied);
        assert_eq!(state.rules[0].conditions[0], ConditionState::Satisfied);
        assert!(
            state.degraded_reasons.is_empty(),
            "the extends-chain merge succeeded here — no lookup failed, so nothing should be \
             reported as degraded: {:?}",
            state.degraded_reasons
        );
    }

    /// Regression for `classify_failure`'s companion gap: a condition
    /// referencing a genuinely-inherited field (declared only on an ancestor
    /// schema, not redeclared) must be classified `NotYetMet`, not
    /// misclassified as `Unresolvable` ("likely a typo"). Before the fix,
    /// `walk_path_against_schema` built `known_fields` from the subtype's own
    /// directly-declared fields alone (empty here), so `status` looked like
    /// neither a field nor a relationship and was reported as a typo.
    ///
    /// Uses a `node_created` trigger — candidate lookup already finds this
    /// rule correctly on an exact `node_type` match — isolating this
    /// assertion from the separate `property_changed` candidate-enumeration
    /// gap covered above.
    #[tokio::test]
    async fn inherited_field_condition_reports_not_yet_met_not_unresolvable() {
        let (svc, _tmp) = test_service().await;

        crate::schema::handle_create_schema(
            &svc,
            json!({
                "name": "wf_base_cf",
                "fields": [
                    { "name": "status", "type": "string", "protection": "user", "indexed": false }
                ]
            }),
        )
        .await
        .expect("base schema creation failed");

        crate::schema::handle_create_schema(
            &svc,
            json!({
                "name": "wf_sub_cf",
                "extends": "wf_base_cf",
                "fields": []
            }),
        )
        .await
        .expect("subtype schema creation failed");

        let lifecycle = Arc::new(RwLock::new(PlaybookLifecycleManager::new()));
        {
            let mut lm = lifecycle.write().unwrap();
            let play = make_play_node(
                "pb-inherit-cf",
                json!([{
                    "name": "r1",
                    "trigger": { "type": "graph_event", "on": "node_created", "node_type": "wf_sub_cf" },
                    "conditions": ["node.status == 'active'"],
                    "actions": []
                }]),
            );
            lm.activate_play(&play).unwrap();
        }

        let task = make_test_node("wf_sub_cf", json!({}));
        let state = get_workflow_state(&lifecycle, &svc, &task).await;
        assert_eq!(state.rules.len(), 1);
        match &state.rules[0].conditions[0] {
            ConditionState::NotYetMet { condition } => {
                assert_eq!(condition, "node.status == 'active'");
            }
            other => panic!(
                "expected NotYetMet for a genuinely inherited field, got {:?} — inherited \
                 fields must not be misclassified as a typo",
                other
            ),
        }
    }

    /// Regression for the same classify_failure gap as above, but for a
    /// declared *relationship* rather than a field: a condition traversing a
    /// relationship declared only on an ancestor schema (inherited, not
    /// redeclared) must be classified `NotYetMet`, not misclassified as
    /// `Unresolvable`. Before the fix, `walk_path_against_schema`'s
    /// relationship lookup checked only the subtype's own directly-declared
    /// relationships (none here), so `story` looked like neither a field nor
    /// a relationship and was reported as a typo.
    #[tokio::test]
    async fn inherited_relationship_condition_reports_not_yet_met_not_unresolvable() {
        let (svc, _tmp) = test_service().await;

        crate::schema::handle_create_schema(
            &svc,
            json!({
                "name": "wf_rel_target",
                "fields": [
                    { "name": "status", "type": "string", "protection": "user", "indexed": false }
                ]
            }),
        )
        .await
        .expect("relationship target schema creation failed");

        crate::schema::handle_create_schema(
            &svc,
            json!({
                "name": "wf_rel_base",
                "fields": [],
                "relationships": [{
                    "name": "story",
                    "targetType": "wf_rel_target",
                    "direction": "out",
                    "cardinality": "one",
                    "reverseName": "tasks",
                    "reverseCardinality": "many"
                }]
            }),
        )
        .await
        .expect("base schema creation failed");

        crate::schema::handle_create_schema(
            &svc,
            json!({
                "name": "wf_rel_sub",
                "extends": "wf_rel_base",
                "fields": []
            }),
        )
        .await
        .expect("subtype schema creation failed");

        let lifecycle = Arc::new(RwLock::new(PlaybookLifecycleManager::new()));
        {
            let mut lm = lifecycle.write().unwrap();
            let play = make_play_node(
                "pb-inherit-rel",
                json!([{
                    "name": "r1",
                    "trigger": { "type": "graph_event", "on": "node_created", "node_type": "wf_rel_sub" },
                    "conditions": ["node.story.status == 'active'"],
                    "actions": []
                }]),
            );
            lm.activate_play(&play).unwrap();
        }

        let task = make_test_node("wf_rel_sub", json!({}));
        let state = get_workflow_state(&lifecycle, &svc, &task).await;
        assert_eq!(state.rules.len(), 1);
        match &state.rules[0].conditions[0] {
            ConditionState::NotYetMet { condition } => {
                assert_eq!(condition, "node.story.status == 'active'");
            }
            other => panic!(
                "expected NotYetMet for a genuinely inherited relationship, got {:?} — \
                 inherited relationships must not be misclassified as a typo",
                other
            ),
        }
    }

    /// Regression for the scheduled/cron candidate gap: a scheduled Play
    /// registered on a base type must still be returned as a candidate for a
    /// subtype node, consistent with the ancestor fan-out graph-event
    /// candidates already get for free via `lookup_rules`. Before the fix,
    /// the cron loop used an exact-string `node_type` match and silently
    /// excluded this rule for every subtype node.
    #[tokio::test]
    async fn scheduled_trigger_on_ancestor_type_is_returned_for_subtype_node() {
        let (svc, _tmp) = test_service().await;
        let lifecycle = Arc::new(RwLock::new(PlaybookLifecycleManager::new()));
        {
            let mut lm = lifecycle.write().unwrap();
            // These tests build the lifecycle manager directly rather than
            // through `PlaybookEngine`, so the ancestor cache — normally kept
            // fresh by `PlaybookEngine::refresh_ancestor_cache` — must be
            // seeded by hand to exercise the fan-out path at all.
            lm.set_ancestor_cache(std::collections::HashMap::from([(
                "wf_sub_cron".to_string(),
                vec!["wf_sub_cron".to_string(), "wf_base_cron".to_string()],
            )]));
            let play = make_play_node(
                "pb-cron-ancestor",
                json!([{
                    "name": "r1",
                    "trigger": { "type": "scheduled", "cron": "0 9 * * *", "node_type": "wf_base_cron" },
                    "conditions": ["node.id != ''"],
                    "actions": []
                }]),
            );
            lm.activate_play(&play).unwrap();
        }

        let node = make_test_node("wf_sub_cron", json!({}));
        let state = get_workflow_state(&lifecycle, &svc, &node).await;
        assert_eq!(
            state.rules.len(),
            1,
            "expected the base-type scheduled rule to be returned as a candidate for the \
             subtype node — before the fix this was silently 0"
        );
        assert!(state.rules[0].all_conditions_satisfied);
    }
}
