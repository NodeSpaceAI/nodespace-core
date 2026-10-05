//! Plays that ship with the product (ADR-060 §8, ADR-079).
//!
//! A core Play is seeded as an ordinary, user-visible play node carrying
//! `properties._seed.default_rules` — the same DB-seeded, user-modifiable
//! pattern prompts and skills use. It can be inspected, edited, disabled or
//! reset to its shipped default like any other play; see [`super::seeded`].
//!
//! Core Plays are one of the seed tables (ADR-086 §10): each is a
//! [`NodeTemplate`] reconciled by its fixed id on every open, like a seeded
//! skill. A Play added in a later release reaches an existing database, a
//! shipped change replaces a Play nobody edited, and a Play the user edited
//! is kept, with the shipped change recorded for them to decide on (ADR-072,
//! ADR-094 §8). A Play has no body, so its one aspect is its config.

use crate::markdown::{prepare_nodes_from_template, NodeTemplate, SeedTier};
use crate::services::error::NodeServiceError;
use crate::services::NodeService;
use serde_json::json;

/// Node id of the parent-task completion rollup Play (ADR-079).
///
/// A fixed literal UUID rather than one minted per install: ADR-060 §5 keys
/// cross-Play rule ordering on the Play node id, so a random id would make two
/// devices order the same rules differently. A seeded node's identity is its
/// id (ADR-086 §10).
pub const PARENT_TASK_COMPLETION_PLAY_ID: &str = "5dc9b580-8840-4d02-b89d-9aa16ac552fd";

// The rules of the spec, plan and decision model (ADR-092 §6). Each id is a
// fixed literal UUID for the reason given on
// [`PARENT_TASK_COMPLETION_PLAY_ID`].
/// Node id of the spec approval Play.
pub const SPEC_APPROVAL_PLAY_ID: &str = "3d139d11-c3b7-42d5-b638-f2a2b41e1674";
/// Node id of the plan approval Play.
pub const PLAN_APPROVAL_PLAY_ID: &str = "9390871a-fcf9-49d6-82ac-947696933cb3";
/// Node id of the task lineage Play.
pub const TASK_LINEAGE_PLAY_ID: &str = "70cfa06c-883d-4a9b-be3f-069d7a1df8f3";
/// Node id of the task blockers Play.
pub const TASK_BLOCKERS_PLAY_ID: &str = "d9ea23d8-9e20-4cfa-9abe-6c86f2164765";
/// Node id of the task criteria Play.
pub const TASK_CRITERIA_PLAY_ID: &str = "95796b4d-f078-44f8-807b-8a2e92167a2d";
/// Node id of the superseded lock Play.
pub const SUPERSEDED_LOCK_PLAY_ID: &str = "54159daf-f37d-47f9-a337-bf644d1f0db2";

/// The id of every Play that ships with the product: the core play table, in
/// the order the plays are seeded.
pub const CORE_PLAY_IDS: &[&str] = &[
    PARENT_TASK_COMPLETION_PLAY_ID,
    SPEC_APPROVAL_PLAY_ID,
    PLAN_APPROVAL_PLAY_ID,
    TASK_LINEAGE_PLAY_ID,
    TASK_BLOCKERS_PLAY_ID,
    TASK_CRITERIA_PLAY_ID,
    SUPERSEDED_LOCK_PLAY_ID,
];

/// The rollup rule, as shipped (ADR-079).
///
/// Trigger, condition and action in one rule:
///
/// - **Trigger** — a `task`'s `status` changes. Registered against `task`, so
///   per ADR-078 it also fires for any type extending `task`, and the
///   condition is evaluated at `task`'s scope: an extending type's own status
///   vocabulary resolves through `maps_to` before comparison, and its own
///   fields are not visible. A Play written here keeps working when an `issue`
///   type appears, without knowing `issue` exists.
/// - **Condition** — walk `child_of` to the parent, then back down its
///   `has_child` children, and require every one to be finished. `cancelled`
///   counts as finished alongside `done`: the question is whether any work
///   remains under the parent, not whether everything succeeded. A parent with
///   no children never completes, because an empty collection evaluates to
///   `false` here — including under `.all()` — so no explicit count guard is
///   needed.
///
/// **Single-parent assumption.** `child_of` is the outline's parent edge, and
/// the whole hierarchy is single-parent by construction: `SqliteStore::get_parent`
/// and `get_parent_id` both resolve it with `LIMIT 1`, so no read path has ever
/// contemplated a second one. A node holding two `has_child` parents is already
/// malformed with respect to that model — nothing in the product creates one —
/// and this Play no-ops there rather than picking a parent arbitrarily: the
/// resolver yields a `Collection` for a multi-row walk, `.has_child` cannot
/// continue from it, and the condition is simply false. That is the desired
/// failure: declining to act on a malformed hierarchy, not silently completing
/// whichever parent happened to sort first.
/// - **Action** — set the parent's `status` to `done`. Never `cancelled`: a
///   parent whose children were all cancelled has completed as a unit of work,
///   and propagating `cancelled` upward would assert an intent this Play has no
///   basis to claim.
///
/// The rule is `reactive`, not `invariant`. ADR-060 §2 restricts invariants to
/// "non-chaining, depth 1", and this rule chains by construction: its own write
/// to the parent is itself a `task` status change, which re-fires the rule with
/// the parent now in the child position. That is how the rollup reaches a
/// grandparent, and it is bounded by the engine's chain-depth cap.
pub fn parent_task_completion_rules() -> serde_json::Value {
    json!([{
        "name": "complete-parent-when-all-children-done",
        "description": "Mark a task done once every one of its sub-tasks is done or cancelled",
        "class": "reactive",
        "trigger": {
            "type": "graph_event",
            "on": "property_changed",
            "select": { "target_type": "task" },
            "property_key": "task.status"
        },
        "conditions": [{
            "expr": "node.child_of.has_child.all(c, c.status == 'done' || c.status == 'cancelled')",
            "description": "Every sub-task of the task's parent is done or cancelled"
        }],
        "actions": [{
            "action_type": "update_node",
            "description": "Mark the parent task done",
            "params": {
                "node_id": "{trigger.node.child_of.id}",
                "properties": { "status": "done" }
            }
        }]
    }])
}

/// A play as shipped, carrying its own default for reset (ADR-060 §8).
fn seeded_play(id: &str, title: &str, description: &str, rules: serde_json::Value) -> NodeTemplate {
    NodeTemplate {
        id: id.to_string(),
        title: title.to_string(),
        markdown_content: String::new(),
        root_node_type: "play".to_string(),
        root_properties: json!({
            "rules": rules,
            // Stated, not left to the schema default: a reset or a taken
            // update replaces what the template names, so every field a
            // user can change is named here.
            "enabled": true,
            "description": description,
            "_seed": { "default_rules": rules },
        }),
        child_node_type: None,
        tier: SeedTier::System,
    }
}

fn parent_task_completion_play() -> NodeTemplate {
    seeded_play(
        PARENT_TASK_COMPLETION_PLAY_ID,
        "Complete a parent task when all its children are done",
        "When every sub-task of a task is done or cancelled, mark the parent done. Reactive \
         rules currently fire only for changes made on this device.",
        parent_task_completion_rules(),
    )
}

// ---------------------------------------------------------------------------
// The spec, plan and decision rules (ADR-092 §6)
// ---------------------------------------------------------------------------
//
// Every rule here is an invariant that rejects: it runs inside the write's
// transaction and vetoes it (ADR-060 §2). A reactive rule could only
// complain after the fact.
//
// Rules fire on transitions, not on links. A node and its relationships are
// separate writes, so a task is created first and linked afterwards, and can
// sit in `open` while its links are assembled. What is guarded is the moment
// work advances: each rule reads the links as they stand when a status
// changes. No rule here watches a link being added or removed.
//
// A trigger's `property_key` is namespaced (`task.status`): that is the form
// an update reports, and a bare key never fires. A rule registered against a
// type also fires for every type extending it (ADR-078).
//
// A read that can find no value is `has()`-guarded: a related node's status,
// and a node's own status once it has been cleared. A missing key fails the
// whole condition, which on a reject rule means "allow", silently.
//
// A field lock reads the status as the write leaves it, so one write that
// both edits a field and supersedes the node is refused like an edit made
// afterwards: supersede in a write of its own.

/// Whether a node has no checkbox child. A checkbox is the only type that
/// derives `checked` (ADR-094 §5), and on any other child it reads as `null`,
/// so a child that is either checked or unchecked is a checkbox. Only direct
/// children are seen: `has_child` is one level.
const NO_CHECKBOX_CHILD: &str =
    "!has(node.has_child) || !node.has_child.exists(c, c.checked == true || c.checked == false)";

/// Whether a task is linked into the model: it carries out a plan, or is
/// governed by at least one spec.
const HAS_PLAN_OR_SPEC: &str = "has(node.plan) || (has(node.spec) && size(node.spec) > 0)";

/// One invariant rule that rejects a property change.
fn reject_change(
    name: &str,
    description: &str,
    node_type: &str,
    field: &str,
    conditions: serde_json::Value,
    action_description: &str,
    message: &str,
) -> serde_json::Value {
    json!({
        "name": name,
        "description": description,
        "class": "invariant",
        "trigger": {
            "type": "graph_event",
            "on": "property_changed",
            "select": { "target_type": node_type },
            "property_key": format!("{node_type}.{field}"),
        },
        "conditions": conditions,
        "actions": [{
            "action_type": "reject",
            "description": action_description,
            "params": { "message": message },
        }],
    })
}

/// Refuse to approve a spec that does not say how done is judged.
///
/// A spec's success criteria are its direct checkbox children (ADR-092 §2),
/// so a spec with none has no criteria to approve.
pub fn spec_approval_rules() -> serde_json::Value {
    json!([reject_change(
        "reject-approving-a-spec-without-criteria",
        "Refuse to approve a spec that has no success criteria",
        "spec",
        "spec_status",
        json!([
            {
                "expr": "node.spec_status == 'approved'",
                "description": "The spec is being approved",
            },
            {
                "expr": NO_CHECKBOX_CHILD,
                "description": "The spec has no checkbox child",
            },
        ]),
        "Refuse the approval and say the spec needs success criteria first",
        "This spec cannot be approved until it has success criteria. Add each criterion as \
         a checkbox directly under the spec, then approve it.",
    )])
}

/// Refuse to approve a plan whose spec is not approved.
///
/// Two rules, because a plan can reach `approved` two ways. Approving an
/// existing plan is a `plan_status` change, checked against the linked spec.
/// Creating a plan already approved can never pass, since at `node_created`
/// it has no `spec` link yet, so that rule rejects outright and says why.
pub fn plan_approval_rules() -> serde_json::Value {
    json!([
        reject_change(
            "reject-approving-a-plan-without-an-approved-spec",
            "Refuse to approve a plan whose spec is not approved",
            "plan",
            "plan_status",
            json!([
                {
                    "expr": "node.plan_status == 'approved'",
                    "description": "The plan is being approved",
                },
                {
                    "expr":
                        "!has(node.spec) || !has(node.spec.spec_status) \
                         || node.spec.spec_status != 'approved'",
                    "description": "The plan has no spec, or its spec is not approved",
                },
            ]),
            "Refuse the approval and say the plan needs an approved spec first",
            "This plan cannot be approved until it is linked to an approved spec. Link it to \
             the spec it implements, and approve that spec first.",
        ),
        {
            "name": "reject-creating-an-approved-plan",
            "description": "Refuse to create a plan that is already approved",
            "class": "invariant",
            "trigger": {
                "type": "graph_event",
                "on": "node_created",
                "select": { "target_type": "plan" },
            },
            "conditions": [{
                "expr": "node.plan_status == 'approved'",
                "description": "The new plan is already approved",
            }],
            "actions": [{
                "action_type": "reject",
                "description": "Refuse the create and say a plan starts as a draft",
                "params": {
                    "message":
                        "A plan cannot be created already approved: approval requires a link \
                         to an approved spec, and a new plan has none yet. Create it as draft, \
                         link it to its spec, then approve it.",
                },
            }],
        },
    ])
}

/// Refuse to advance a planned task whose lineage is incomplete.
///
/// Fires on the move into `in_progress`, `in_review` or `done`. A task in
/// review has been started, so the move to `in_review` is guarded like the
/// move to `in_progress` (ADR-092 §5). `cancelled` is always allowed:
/// abandoning work needs no approval.
///
/// Scoped to tasks with a `plan`: a task with none is never checked. A
/// planned task's plan must be approved, and the task must link the spec
/// that plan implements, not merely some spec.
pub fn task_lineage_rules() -> serde_json::Value {
    json!([reject_change(
        "reject-advancing-a-planned-task-without-lineage",
        "Refuse to start, review or finish a planned task until its plan is approved and it \
         links the plan's spec",
        "task",
        "status",
        json!([
            {
                "expr":
                    "node.status == 'in_progress' || node.status == 'in_review' \
                     || node.status == 'done'",
                "description": "The task is being started, put in review or finished",
            },
            {
                "expr": "has(node.plan)",
                "description": "The task carries out a plan",
            },
            {
                "expr":
                    "!has(node.plan.plan_status) || node.plan.plan_status != 'approved' \
                     || !has(node.plan.spec) || !has(node.spec) \
                     || !node.spec.exists(s, s == node.plan.spec)",
                "description":
                    "The plan is not approved, or the task is not linked to the spec the plan \
                     implements",
            },
        ]),
        "Refuse the change and say the plan must be approved and the task linked to its spec",
        "This task carries out a plan, so it cannot be started, put in review or finished \
         until that plan is approved and the task is linked to the plan's spec as well. \
         Approve the plan, and link the task to the spec the plan implements.",
    )])
}

/// Refuse to start a task while something blocking it is unfinished.
///
/// Applies to every task, with or without a plan or spec. Uses the
/// `blocked_by` reverse edge of the task schema's `blocks` declaration. A
/// cancelled blocker no longer blocks. The move to `in_review` is guarded
/// like the move to `in_progress` (ADR-092 §5); a blocked task may sit in
/// `open`, and may be cancelled.
pub fn task_blockers_rules() -> serde_json::Value {
    json!([reject_change(
        "reject-starting-a-blocked-task",
        "Refuse to start a task, or put it in review, while a task blocking it is unfinished",
        "task",
        "status",
        json!([
            {
                "expr": "node.status == 'in_progress' || node.status == 'in_review'",
                "description": "The task is being started or put in review",
            },
            {
                "expr": "node.blocked_by.exists(b, b.status != 'done' && b.status != 'cancelled')",
                "description": "At least one blocking task is neither done nor cancelled",
            },
        ]),
        "Refuse the change and say the blocking task must be finished first",
        "This task is blocked by a task that is not finished yet. Finish or cancel the \
         blocking task first, or remove the blocks relationship if it no longer applies.",
    )])
}

/// Refuse to finish a task whose checklist is incomplete.
///
/// A task's acceptance criteria are its direct checkbox children (ADR-092
/// §2). Two rules:
///
/// - A task with an unchecked checkbox child cannot be done. This applies to
///   every task: one that has a checklist finishes it before it closes.
/// - A task linked to a plan or a spec cannot be done with no checklist at
///   all. A task outside the model needs none.
pub fn task_criteria_rules() -> serde_json::Value {
    json!([
        reject_change(
            "reject-done-with-an-unchecked-criterion",
            "Refuse to mark a task done while an item of its checklist is unchecked",
            "task",
            "status",
            json!([
                {
                    "expr": "node.status == 'done'",
                    "description": "The task is being marked done",
                },
                {
                    "expr": "node.has_child.exists(c, c.checked == false)",
                    "description": "At least one checkbox child is not checked",
                },
            ]),
            "Refuse the change and say the checklist must be completed first",
            "This task cannot be marked done while its checklist has an unchecked item. Check \
             each item once it is met, or remove an item that no longer applies.",
        ),
        reject_change(
            "reject-done-without-criteria",
            "Refuse to mark a task under a plan or spec done when it has no checklist",
            "task",
            "status",
            json!([
                {
                    "expr": "node.status == 'done'",
                    "description": "The task is being marked done",
                },
                {
                    "expr": HAS_PLAN_OR_SPEC,
                    "description": "The task is linked to a plan or a spec",
                },
                {
                    "expr": NO_CHECKBOX_CHILD,
                    "description": "The task has no checkbox child",
                },
            ]),
            "Refuse the change and say the task needs a checklist first",
            "This task is linked to a plan or a spec, so it cannot be marked done without \
             acceptance criteria. Add each criterion as a checkbox directly under the task \
             and check it once it is met.",
        ),
    ])
}

/// The fields a superseded node may no longer change, per type, beside its
/// status field. A decision's status is its only field.
const LOCKED_FIELDS: [(&str, &str, &[&str]); 3] = [
    ("spec", "spec_status", &["objective", "boundaries"]),
    ("plan", "plan_status", &["approach", "risks"]),
    ("decision", "decision_status", &[]),
];

/// Freeze a superseded spec, plan or decision.
///
/// One rule per field, because a trigger names one property. The field rules
/// do not trigger on the status field itself: the change INTO `superseded`
/// must pass, and a rule reading post-write state cannot tell that change
/// from an edit made afterwards. The change back OUT of `superseded` is
/// rejected separately, from the old value, or the lock would be one status
/// flip away from not being a lock.
pub fn superseded_lock_rules() -> serde_json::Value {
    let mut rules = Vec::new();
    for (node_type, status_field, fields) in LOCKED_FIELDS {
        for field in fields {
            rules.push(reject_change(
                &format!("lock-superseded-{node_type}-{field}"),
                &format!("Refuse a change to a superseded {node_type}'s {field}"),
                node_type,
                field,
                json!([{
                    "expr": format!("node.{status_field} == 'superseded'"),
                    "description": format!("The {node_type} is superseded"),
                }]),
                &format!("Refuse the change and say a superseded {node_type}'s {field} is locked"),
                // Names the field, which also keeps each rule's action list
                // distinct: byte-identical actions in one play derive the
                // same rule identity and are refused.
                &format!(
                    "This {node_type} is superseded, so its {field} is locked as the record of \
                     what was agreed. Create a new {node_type} for the revised version instead \
                     of editing this one."
                ),
            ));
        }
        rules.push(reject_change(
            &format!("lock-superseded-{node_type}-status"),
            &format!("Refuse to move a superseded {node_type} to another status"),
            node_type,
            status_field,
            json!([
                {
                    "expr": "trigger.property.old_value == 'superseded'",
                    "description": format!("The {node_type} was superseded before this change"),
                },
                {
                    // Clearing the status leaves it with none, which reads
                    // as the default: that is leaving `superseded` too.
                    "expr": format!(
                        "!has(node.{status_field}) || node.{status_field} != 'superseded'"
                    ),
                    "description": "The change leaves it with another status, or with none",
                },
            ]),
            &format!("Refuse the change and say a superseded {node_type} cannot be reinstated"),
            &format!(
                "A superseded {node_type} cannot be reinstated. Create a new {node_type} instead."
            ),
        ));
    }
    serde_json::Value::Array(rules)
}

/// Every Play that ships with the product: the core play seed table.
pub fn core_play_templates() -> Vec<NodeTemplate> {
    vec![
        parent_task_completion_play(),
        seeded_play(
            SPEC_APPROVAL_PLAY_ID,
            "Require success criteria before approving a spec",
            "Rejects approving a spec that has no checkbox child. A spec's success criteria \
             are the checkboxes directly under it.",
            spec_approval_rules(),
        ),
        seeded_play(
            PLAN_APPROVAL_PLAY_ID,
            "Require an approved spec before approving a plan",
            "Rejects approving a plan unless it is linked to an approved spec, and rejects \
             creating a plan that is already approved.",
            plan_approval_rules(),
        ),
        seeded_play(
            TASK_LINEAGE_PLAY_ID,
            "Require an approved plan and its spec before advancing a planned task",
            "Rejects moving a task that carries out a plan to in_progress, in_review or done \
             unless that plan is approved and the task is also linked to the plan's spec. A \
             task with no plan is never checked.",
            task_lineage_rules(),
        ),
        seeded_play(
            TASK_BLOCKERS_PLAY_ID,
            "Block starting a task that is blocked",
            "Rejects moving a task to in_progress or in_review while a task it is blocked by \
             is neither done nor cancelled. Applies to every task.",
            task_blockers_rules(),
        ),
        seeded_play(
            TASK_CRITERIA_PLAY_ID,
            "Require a completed checklist before finishing a task",
            "Rejects marking a task done while a checkbox directly under it is unchecked, and \
             rejects marking a task that is linked to a plan or a spec done when it has no \
             checkbox at all. A task with no plan, no spec and no checklist is never checked.",
            task_criteria_rules(),
        ),
        seeded_play(
            SUPERSEDED_LOCK_PLAY_ID,
            "Lock superseded specs, plans and decisions",
            "Rejects edits to the fields of a superseded spec, plan or decision, and rejects \
             moving it back out of superseded. A revision is a new node, so anything that \
             referenced the old one still reads what it was built against.",
            superseded_lock_rules(),
        ),
    ]
}

/// Reconcile the core Plays with their seed table.
///
/// Runs on every open, through the reconciliation every seeded kind uses
/// ([`NodeService::seed_nodes_from_templates`]): a missing Play is created
/// under its fixed id, a shipped change replaces a Play nobody edited, and a
/// Play the user edited or switched off is never overwritten.
pub async fn seed_core_plays(service: &NodeService) -> Result<(), NodeServiceError> {
    let mut groups = Vec::new();
    for template in core_play_templates() {
        let nodes = prepare_nodes_from_template(&template).map_err(|e| {
            NodeServiceError::invalid_update(format!(
                "core play '{}' does not expand: {e}",
                template.title
            ))
        })?;
        groups.push(nodes);
    }
    service.seed_nodes_from_templates(groups).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::playbook::types::parse_rules_from_properties;

    #[test]
    fn the_shipped_rules_parse() {
        for (rules, count) in [
            (parent_task_completion_rules(), 1),
            (spec_approval_rules(), 1),
            (plan_approval_rules(), 2),
            (task_lineage_rules(), 1),
            (task_blockers_rules(), 1),
            (task_criteria_rules(), 2),
            // Two fields and a status for a spec and for a plan, and a
            // status for a decision.
            (superseded_lock_rules(), 7),
        ] {
            let props = json!({ "play": { "rules": rules } });
            let rules = parse_rules_from_properties(&props).expect("shipped rules must parse");
            assert_eq!(rules.len(), count);
        }
    }

    /// ADR-060 §2: an invariant must be non-chaining, and this rule chains by
    /// construction. Declaring it invariant would be rejected at save time.
    #[test]
    fn the_rollup_rule_is_reactive() {
        let rules = parent_task_completion_rules();
        assert_eq!(
            rules[0]["class"], "reactive",
            "the rollup chains, so it cannot be an invariant"
        );
    }

    /// Each seeded node must carry its own shipped default, or ADR-060 §8's
    /// reset has nothing to restore from.
    #[test]
    fn every_seeded_play_carries_its_default_rules() {
        for play in core_play_templates() {
            assert!(
                crate::models::PlayFields::seeded_in(&play.root_properties),
                "{}: a core Play must be marked as seeded",
                play.id
            );
            assert_eq!(
                play.root_properties["_seed"]["default_rules"], play.root_properties["rules"],
                "{}: the stored default must match the shipped rules",
                play.id
            );
        }
    }

    /// The spec, plan and decision rules veto a write, which only an
    /// invariant can do: a reject on a reactive rule is refused at save time.
    #[test]
    fn every_model_rule_is_an_invariant_reject() {
        for play in core_play_templates() {
            if play.id == PARENT_TASK_COMPLETION_PLAY_ID {
                continue;
            }
            for rule in play.root_properties["rules"].as_array().expect("rules") {
                assert_eq!(rule["class"], "invariant", "{}", rule["name"]);
                assert_eq!(rule["actions"][0]["action_type"], "reject");
            }
        }
    }

    /// The lock must never check its own status field against post-write
    /// state alone, or the move INTO superseded would be rejected.
    #[test]
    fn the_status_rules_of_the_lock_read_the_old_value() {
        let rules = superseded_lock_rules();
        let mut status_rules = 0;
        for rule in rules.as_array().unwrap() {
            let key = rule["trigger"]["property_key"].as_str().unwrap();
            if key.ends_with("_status") {
                status_rules += 1;
                assert!(
                    rule["conditions"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .any(|c| c["expr"].as_str().unwrap().contains("old_value")),
                    "{key}: a post-write-only check would reject superseding itself"
                );
            }
        }
        assert_eq!(status_rules, 3, "one status rule per locked type");
    }

    /// The lock covers every field the three schemas declare: a field added
    /// to one of them without a lock rule fails here.
    #[test]
    fn the_lock_covers_every_field_of_the_three_types() {
        let schemas = crate::models::core_schemas::get_core_schemas();
        let locked: std::collections::HashSet<String> = superseded_lock_rules()
            .as_array()
            .unwrap()
            .iter()
            .map(|rule| {
                rule["trigger"]["property_key"]
                    .as_str()
                    .unwrap()
                    .to_string()
            })
            .collect();
        for (node_type, _, _) in LOCKED_FIELDS {
            let schema = schemas
                .iter()
                .find(|s| s.envelope.id == node_type)
                .unwrap_or_else(|| panic!("{node_type} is a core schema"));
            for field in &schema.fields {
                let key = format!("{node_type}.{}", field.name);
                assert!(locked.contains(&key), "{key} is not locked");
            }
        }
    }

    /// The lineage and blocker rules guard the move to `in_review` as well
    /// as `in_progress` (ADR-092 §5).
    #[test]
    fn starting_rules_guard_in_review_too() {
        for rules in [task_lineage_rules(), task_blockers_rules()] {
            let expr = rules[0]["conditions"][0]["expr"].as_str().unwrap();
            assert!(expr.contains("'in_progress'"), "{expr}");
            assert!(expr.contains("'in_review'"), "{expr}");
        }
    }

    /// Every core play ships a description on each rule, condition and action
    /// (ADR-090 §1), in its live rules and in its reset target alike.
    #[test]
    fn every_core_play_describes_its_rules_conditions_and_actions() {
        for play in core_play_templates() {
            for rules in [
                &play.root_properties["rules"],
                &play.root_properties["_seed"]["default_rules"],
            ] {
                let rules = parse_rules_from_properties(&json!({ "rules": rules }))
                    .unwrap_or_else(|e| panic!("{}: {e}", play.id));
                assert!(!rules.is_empty(), "{} has no rules", play.id);
                if let Err(errors) = crate::playbook::descriptions::check_descriptions(&rules, None)
                {
                    panic!("{}: {errors:?}", play.id);
                }
            }
        }
    }

    /// An id is load-bearing for cross-device rule ordering (ADR-060 §5), so
    /// none may drift.
    #[test]
    fn the_play_ids_are_stable_and_distinct() {
        assert_eq!(
            parent_task_completion_play().id,
            PARENT_TASK_COMPLETION_PLAY_ID
        );
        assert_eq!(
            CORE_PLAY_IDS,
            [
                "5dc9b580-8840-4d02-b89d-9aa16ac552fd",
                "3d139d11-c3b7-42d5-b638-f2a2b41e1674",
                "9390871a-fcf9-49d6-82ac-947696933cb3",
                "70cfa06c-883d-4a9b-be3f-069d7a1df8f3",
                "d9ea23d8-9e20-4cfa-9abe-6c86f2164765",
                "95796b4d-f078-44f8-807b-8a2e92167a2d",
                "54159daf-f37d-47f9-a337-bf644d1f0db2",
            ]
        );
        let unique: std::collections::HashSet<_> = CORE_PLAY_IDS.iter().collect();
        assert_eq!(unique.len(), CORE_PLAY_IDS.len());
    }

    /// The id table names exactly the plays that are seeded, in order, and
    /// each is a UUID: no play id is a slug.
    #[test]
    fn the_id_table_matches_the_seeded_plays() {
        let seeded: Vec<String> = core_play_templates()
            .into_iter()
            .map(|play| play.id)
            .collect();
        assert_eq!(seeded, CORE_PLAY_IDS);
        for id in CORE_PLAY_IDS {
            assert!(uuid::Uuid::parse_str(id).is_ok(), "{id} is not a UUID");
        }
    }

    mod integration {
        use super::*;
        use crate::db::SqliteStore;
        use crate::models::NodeUpdate;
        use std::sync::Arc;
        use tempfile::TempDir;

        async fn create_test_service() -> (Arc<NodeService>, TempDir) {
            let temp_dir = TempDir::new().unwrap();
            let db_path = temp_dir.path().join("test.db");
            let mut store: Arc<SqliteStore> = Arc::new(SqliteStore::new(db_path).await.unwrap());
            let node_service = Arc::new(NodeService::new(&mut store).await.unwrap());
            (node_service, temp_dir)
        }

        /// Seeding never overwrites an existing Play node, so a user's edits to
        /// a core Play survive the next open.
        #[tokio::test]
        async fn re_seeding_keeps_an_existing_plays_edits() {
            let (service, _temp) = create_test_service().await;

            let existing = service
                .get_node(PARENT_TASK_COMPLETION_PLAY_ID)
                .await
                .unwrap()
                .expect("the core Play is seeded");
            service
                .update_node(
                    PARENT_TASK_COMPLETION_PLAY_ID,
                    existing.version,
                    NodeUpdate::default().with_properties(json!({
                        "description": "A description the user wrote."
                    })),
                )
                .await
                .unwrap();

            seed_core_plays(&service).await.unwrap();

            let stored = service
                .get_node(PARENT_TASK_COMPLETION_PLAY_ID)
                .await
                .unwrap()
                .unwrap();
            // A persisted Play keeps its properties under the `play` namespace.
            assert_eq!(
                stored.properties["play"]["description"],
                "A description the user wrote."
            );
        }
    }
}
