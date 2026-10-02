//! Spec-driven development: a spec → plan → task lineage, gated by approval.
//!
//! Models the invariant shared by every spec-driven tool worth copying —
//! GitHub's Spec Kit, AWS Kiro, and the published agent-skill versions of the
//! pattern: work flows one way from a spec (what and why) through a plan (how)
//! to tasks (the work), and nothing advances past a stage that has not been
//! approved. As with every playbook, nothing here is platform code; it is a
//! composition of schemas, declared relationships, one added field, and Plays.
//!
//! # Lineage is declared relationships, not `has_child` and not mentions
//!
//! `has_child` is outline nesting, and a task implementing a plan is not
//! nested inside it. Mentions are content-derived and deliberately
//! schema-free — they stay available for informal context, but a gate cannot
//! be built on them: the Play resolver walks declared relationships, and a
//! mention exposes only an id, never the mentioned node's status. So every
//! link a gate reads is a declared relationship, visible in the schema itself.
//!
//! All three are declared on the new types and point at `task`, as the Linear
//! playbook's `cycle.tasks` does, so core `task` gains no relationship
//! declarations — only the reverse names `plan` and `spec` it can be walked by.
//!
//! # Gates fire on transitions, not on creation
//!
//! A node and its relationships are separate writes: a task is created first
//! and linked afterwards, so at `node_created` it has no links to check. And an
//! invariant rule cannot trigger on the link itself — synchronous in-transaction
//! dispatch exists only for `node_created` and `property_changed`, and a
//! relationship write has no such hook. Gating task *creation* on its lineage is
//! therefore not expressible, and a rule claiming to do it would either reject
//! every task or none.
//!
//! What is expressible, and what actually matters, is gating the moments work
//! advances: a plan cannot be approved against an unapproved spec, and a task
//! linked to a plan cannot be started or finished until that plan is approved
//! and the task names its spec. A task can exist in `open` while its lineage is
//! assembled; it cannot move.
//!
//! # Gates are scoped to tasks that opted in
//!
//! Every task rule requires the task to already carry a `plan` or `spec` link.
//! An ordinary task elsewhere in the workspace — a chore, an agent-created
//! reminder — never meets them. Installing this methodology must not make
//! closing an unrelated task require a verification method.

use crate::methodology::skills::playbook_skill;
use crate::methodology::{
    FieldValueExtension, MethodologyPlaybook, PlayStep, SchemaStep, ViewStep,
};
use crate::services::QueryDefinition;
use serde_json::json;

// The fixed ids of the nodes this playbook seeds. A seeded node's identity
// is its id (ADR-086 §10), and ADR-060 §5 orders rules across plays by play
// id, so each is a literal UUID rather than one minted per install.
/// The `spec-driven-plan-approval-gate` play.
pub const PLAN_APPROVAL_GATE_ID: &str = "2f8b6c15-9e04-4d7a-b3c8-6a1e5f7d9b01";
/// The `spec-driven-task-lineage-gate` play.
pub const TASK_LINEAGE_GATE_ID: &str = "2f8b6c15-9e04-4d7a-b3c8-6a1e5f7d9b02";
/// The `spec-driven-task-verification-gate` play.
pub const TASK_VERIFICATION_GATE_ID: &str = "2f8b6c15-9e04-4d7a-b3c8-6a1e5f7d9b03";
/// The `spec-driven-supersession-lock` play.
pub const SUPERSESSION_LOCK_ID: &str = "2f8b6c15-9e04-4d7a-b3c8-6a1e5f7d9b04";
/// The `spec-driven-specs-by-status` saved view.
pub const SPECS_BY_STATUS_ID: &str = "2f8b6c15-9e04-4d7a-b3c8-6a1e5f7d9b05";
/// The `spec-driven-plans-by-status` saved view.
pub const PLANS_BY_STATUS_ID: &str = "2f8b6c15-9e04-4d7a-b3c8-6a1e5f7d9b06";

/// The spec-driven playbook.
pub fn playbook() -> MethodologyPlaybook {
    MethodologyPlaybook {
        id: "spec-driven",
        name: "Spec-driven",
        description:
            "Specs capture what and why, plans capture how, and tasks trace back to both. \
             A plan cannot be approved against an unapproved spec, a planned task cannot start \
             or finish until its plan is approved and it names its spec, a task cannot close \
             without a recorded verification method, and a superseded spec or plan is locked.",
        schemas: vec![spec_schema(), plan_schema()],
        field_value_extensions: vec![verification_method_field()],
        plays: vec![
            plan_approval_gate(),
            task_lineage_gate(),
            task_verification_gate(),
            supersession_lock(),
        ],
        skills: vec![
            playbook_skill(include_str!("skills/spec_driven/writing-a-spec.md")),
            playbook_skill(include_str!("skills/spec_driven/writing-a-plan.md")),
            playbook_skill(include_str!(
                "skills/spec_driven/creating-implementation-tasks.md"
            )),
            playbook_skill(include_str!(
                "skills/spec_driven/completing-a-spec-driven-task.md"
            )),
        ],
        overview: include_str!("skills/spec_driven/spec-driven-workspace.md"),
        views: vec![specs_by_status_view(), plans_by_status_view()],
    }
}

// ---------------------------------------------------------------------------
// Schemas
// ---------------------------------------------------------------------------

/// The approval vocabulary shared by `spec_status` and `plan_status`.
///
/// Not extensible: the gates compare against `approved` and `superseded` by
/// name, so a value added beside them would be one no rule understands.
fn approval_values() -> serde_json::Value {
    json!([
        { "value": "draft", "label": "Draft" },
        { "value": "approved", "label": "Approved" },
        { "value": "superseded", "label": "Superseded" },
    ])
}

/// A free-text field. Every content field in this playbook is prose, never an
/// enum: acceptance criteria and boundaries differ per spec, and a fixed list
/// would fight real usage.
fn text_field(name: &str, friendly_name: &str, description: &str) -> serde_json::Value {
    json!({
        "name": name,
        "friendlyName": friendly_name,
        "type": "text",
        "protection": "user",
        "indexed": false,
        "required": false,
        "description": description,
    })
}

/// `spec` — what is being built and why.
///
/// The status field is `spec_status`, not `status`: a bare `status` would read
/// as the core task lifecycle to every consumer that knows that name, and a
/// spec is not a unit of work.
fn spec_schema() -> SchemaStep {
    SchemaStep {
        schema_id: "spec",
        params: json!({
            "name": "Spec",
            "description":
                "What is being built and why: the objective, the testable criteria that decide \
                 whether it is done, and the boundaries on how it may be done. The source of \
                 truth every plan and task under it is judged against.",
            "fields": [
                text_field(
                    "objective",
                    "Objective",
                    "What is being built, why, and for whom — in the requester's own terms, \
                     not a restatement of the title.",
                ),
                text_field(
                    "success_criteria",
                    "Success criteria",
                    "Testable conditions that decide whether the work is done. Each should \
                     name a check someone could actually run or observe.",
                ),
                text_field(
                    "boundaries",
                    "Boundaries",
                    "What may always be done without asking, what needs sign-off first, and \
                     what must never be done.",
                ),
                {
                    "name": "spec_status",
                    "friendlyName": "Spec status",
                    "type": "enum",
                    "protection": "user",
                    "indexed": true,
                    "required": false,
                    "extensible": false,
                    "default": "draft",
                    "coreValues": approval_values(),
                    "userValues": [],
                    "description":
                        "draft while being written; approved once the requester has confirmed \
                         it, which is what lets a plan against it be approved; superseded once \
                         a newer spec replaces it, which locks its content.",
                },
            ],
            "relationships": [
                {
                    "name": "tasks",
                    "targetType": "task",
                    "direction": "out",
                    "cardinality": "many",
                    "reverseName": "spec",
                    "reverseCardinality": "many",
                    "description":
                        "Tasks this spec governs, linked directly rather than only through a \
                         plan so a task's spec is one hop away. A task may serve more than \
                         one spec.",
                },
            ],
        }),
    }
}

/// `plan` — how an approved spec will be built.
fn plan_schema() -> SchemaStep {
    SchemaStep {
        schema_id: "plan",
        params: json!({
            "name": "Plan",
            "description":
                "The technical approach for one spec: components, sequencing, and risks. \
                 Revisions are new plans; the old one is marked superseded rather than \
                 rewritten, so what a task was built against stays readable.",
            "fields": [
                text_field(
                    "approach",
                    "Approach",
                    "Major components, dependencies and sequencing, tied to the spec's success \
                     criteria by name.",
                ),
                text_field(
                    "risks",
                    "Risks",
                    "What could go wrong with this particular approach.",
                ),
                {
                    "name": "plan_status",
                    "friendlyName": "Plan status",
                    "type": "enum",
                    "protection": "user",
                    "indexed": true,
                    "required": false,
                    "extensible": false,
                    "default": "draft",
                    "coreValues": approval_values(),
                    "userValues": [],
                    "description":
                        "draft while being written; approved once the requester has confirmed \
                         it, which requires its spec to be approved and is what lets planned \
                         tasks start; superseded once a newer plan replaces it, which locks \
                         its content.",
                },
            ],
            "relationships": [
                {
                    "name": "spec",
                    "targetType": "spec",
                    "direction": "out",
                    "cardinality": "one",
                    "reverseName": "plans",
                    "reverseCardinality": "many",
                    "description":
                        "The spec this plan implements. One per plan; a spec accumulates plans \
                         as they are revised and superseded.",
                },
                {
                    "name": "tasks",
                    "targetType": "task",
                    "direction": "out",
                    "cardinality": "many",
                    "reverseName": "plan",
                    "reverseCardinality": "one",
                    "description": "Tasks that carry out this plan. A task belongs to one plan.",
                },
            ],
        }),
    }
}

// ---------------------------------------------------------------------------
// Schema extensions
// ---------------------------------------------------------------------------

/// Add `custom:verification_method` to core `task`.
///
/// Prefixed, per ADR-063: this extends a core type directly rather than through
/// an `extends` subtype, so a bare name could collide with a property a future
/// release gives `task` itself. Conditions read it unprefixed —
/// `node.verification_method` — because the CEL context strips namespaces.
fn verification_method_field() -> FieldValueExtension {
    FieldValueExtension {
        schema_id: "task",
        field: "custom:verification_method",
        params: json!({
            "schema_id": "task",
            "add_fields": [{
                "name": "custom:verification_method",
                "friendlyName": "Verification method",
                "type": "text",
                "protection": "user",
                "indexed": false,
                "required": false,
                "description":
                    "How completion was verified — the command run or the steps checked, and \
                     what they showed. Required before a spec-driven task can be marked done.",
            }],
        }),
    }
}

// ---------------------------------------------------------------------------
// Plays
// ---------------------------------------------------------------------------

/// Whether a task carries this methodology's lineage at all.
///
/// `has()` rather than a bare read: a missing relationship leaves its key absent
/// from the condition context, and reading an absent key fails the whole
/// condition — which on a reject rule means "allow", silently.
const IN_METHODOLOGY: &str = "has(node.plan) || (has(node.spec) && size(node.spec) > 0)";

/// Refuse to approve a plan whose spec is not approved.
///
/// Two rules, because a plan can reach `approved` two ways. Approving an
/// existing plan is a `plan_status` change, checked against the linked spec.
/// Creating a plan already approved can never pass — at `node_created` it has
/// no `spec` link yet — so that rule rejects outright and says why.
fn plan_approval_gate() -> PlayStep {
    PlayStep {
        play_id: PLAN_APPROVAL_GATE_ID,
        name: "Require an approved spec before approving a plan",
        description: "Rejects approving a plan unless it is linked to a spec whose spec_status is \
             approved. Approve the spec first, or link the plan to the spec it implements.",
        rules: json!([
            {
                "name": "reject-approval-without-approved-spec",
                "class": "invariant",
                "trigger": {
                    "type": "graph_event",
                    "on": "property_changed",
                    "select": { "target_type": "plan" },
                    // Namespaced: a trigger's property_key matches the
                    // `{node_type}.{field}` form `update_node` reports.
                    "property_key": "plan.plan_status",
                },
                // Every read of a linked node's status is `has()`-guarded:
                // `bulk_create` applies no schema defaults, so a spec can
                // carry no `spec_status` at all, and an unguarded read of it
                // fails the condition — which lets the approval through.
                "conditions": [
                    "node.plan_status == 'approved'",
                    "!has(node.spec) || !has(node.spec.spec_status) \
                     || node.spec.spec_status != 'approved'",
                ],
                "actions": [{
                    "action_type": "reject",
                    "params": {
                        "message":
                            "This plan cannot be approved until it is linked to an approved \
                             spec. Link it to the spec it implements, and approve that spec \
                             first.",
                    },
                }],
            },
            {
                "name": "reject-creating-an-approved-plan",
                "class": "invariant",
                "trigger": {
                    "type": "graph_event",
                    "on": "node_created",
                    "select": { "target_type": "plan" },
                },
                "conditions": ["node.plan_status == 'approved'"],
                "actions": [{
                    "action_type": "reject",
                    "params": {
                        "message":
                            "A plan cannot be created already approved: approval requires a \
                             link to an approved spec, and a new plan has none yet. Create it \
                             as draft, link it to its spec, then approve it.",
                    },
                }],
            },
        ]),
    }
}

/// Refuse to start or finish a planned task whose lineage is incomplete.
///
/// Fires on the move into `in_progress` or `done` — the moment work advances.
/// A task may sit in `open` while its links are assembled, and `cancelled` is
/// always allowed: abandoning work needs no approval.
///
/// Scoped to tasks with a `plan`: an unplanned task is outside the methodology
/// and never checked. A planned task's plan must be approved, and the task
/// must link the spec that plan implements — not merely some spec. Status
/// reads are `has()`-guarded for the reason given on [`plan_approval_gate`].
fn task_lineage_gate() -> PlayStep {
    PlayStep {
        play_id: TASK_LINEAGE_GATE_ID,
        name: "Require an approved plan and a spec before starting planned work",
        description:
            "Rejects moving a task linked to a plan into in_progress or done unless that plan \
             is approved and the task is also linked to the spec that plan implements.",
        rules: json!([{
            "name": "reject-advancing-without-lineage",
            "class": "invariant",
            "trigger": {
                "type": "graph_event",
                "on": "property_changed",
                "select": { "target_type": "task" },
                "property_key": "task.status",
            },
            "conditions": [
                "node.status == 'in_progress' || node.status == 'done'",
                "has(node.plan)",
                "!has(node.plan.plan_status) || node.plan.plan_status != 'approved' \
                 || !has(node.plan.spec) || !has(node.spec) \
                 || !node.spec.exists(s, s == node.plan.spec)",
            ],
            "actions": [{
                "action_type": "reject",
                "params": {
                    "message":
                        "This task implements a plan, so it cannot start or finish until that \
                         plan is approved and the task is linked to the plan's spec as well. Approve \
                         the plan, and link the task to the spec the plan implements.",
                },
            }],
        }]),
    }
}

/// Refuse to close a spec-driven task with no verification method recorded.
///
/// Presence only — no rule can judge whether "tested" is a real verification.
/// The seeded skill carries that half.
fn task_verification_gate() -> PlayStep {
    PlayStep {
        play_id: TASK_VERIFICATION_GATE_ID,
        name: "Require a verification method before closing spec-driven work",
        description: "Rejects marking a task linked to a plan or spec done until its verification \
             method records how the work was checked.",
        rules: json!([{
            "name": "reject-done-without-verification",
            "class": "invariant",
            "trigger": {
                "type": "graph_event",
                "on": "property_changed",
                "select": { "target_type": "task" },
                "property_key": "task.status",
            },
            "conditions": [
                "node.status == 'done'",
                IN_METHODOLOGY,
                "!has(node.verification_method) || node.verification_method == ''",
            ],
            "actions": [{
                "action_type": "reject",
                "params": {
                    "message":
                        "This task cannot be marked done until its verification method records \
                         how the work was checked. Set custom:verification_method first, then \
                         change the status.",
                },
            }],
        }]),
    }
}

/// The content fields a superseded node may no longer change, per type.
const LOCKED_FIELDS: [(&str, &str, &[&str]); 2] = [
    (
        "spec",
        "spec_status",
        &["objective", "success_criteria", "boundaries"],
    ),
    ("plan", "plan_status", &["approach", "risks"]),
];

/// Freeze a superseded spec or plan.
///
/// One rule per content field, because a trigger names one property. The lock
/// deliberately does not trigger on the status field itself — the change INTO
/// `superseded` must pass, and a rule reading post-write state cannot tell
/// that change from an edit made afterwards. The change back OUT of
/// `superseded` is rejected separately, from the old value, or the lock would
/// be one status flip away from not being a lock.
fn supersession_lock() -> PlayStep {
    let mut rules = Vec::new();
    for (node_type, status_field, fields) in LOCKED_FIELDS {
        for field in fields {
            rules.push(json!({
                "name": format!("lock-superseded-{node_type}-{field}"),
                "class": "invariant",
                "trigger": {
                    "type": "graph_event",
                    "on": "property_changed",
                    "select": { "target_type": node_type },
                    "property_key": format!("{node_type}.{field}"),
                },
                "conditions": [format!("node.{status_field} == 'superseded'")],
                "actions": [{
                    "action_type": "reject",
                    "params": {
                        // Names the field, which also keeps each rule's
                        // action list distinct: byte-identical actions in one
                        // play derive the same rule identity and are refused.
                        "message": format!(
                            "This {node_type} is superseded, so its {field} is locked as the \
                             record of what was agreed. Create a new {node_type} for the \
                             revised version instead of editing this one."
                        ),
                    },
                }],
            }));
        }
        rules.push(json!({
            "name": format!("lock-superseded-{node_type}-status"),
            "class": "invariant",
            "trigger": {
                "type": "graph_event",
                "on": "property_changed",
                "select": { "target_type": node_type },
                "property_key": format!("{node_type}.{status_field}"),
            },
            "conditions": [
                "trigger.property.old_value == 'superseded'",
                format!("node.{status_field} != 'superseded'"),
            ],
            "actions": [{
                "action_type": "reject",
                "params": {
                    "message": format!(
                        "A superseded {node_type} cannot be reinstated. Create a new \
                         {node_type} instead."
                    ),
                },
            }],
        }));
    }

    PlayStep {
        play_id: SUPERSESSION_LOCK_ID,
        name: "Lock superseded specs and plans",
        description:
            "Rejects edits to a superseded spec's or plan's content, and rejects moving it back \
             out of superseded. Revisions are new nodes, so anything that referenced the old \
             one still reads what it was built against.",
        rules: serde_json::Value::Array(rules),
    }
}

// ---------------------------------------------------------------------------
// Views
// ---------------------------------------------------------------------------

/// Specs by approval state — what is being drafted, what is agreed, what has
/// been replaced.
fn specs_by_status_view() -> ViewStep {
    ViewStep {
        view_id: SPECS_BY_STATUS_ID,
        name: "Specs by Status",
        definition: QueryDefinition {
            target_type: "spec".to_string(),
            filters: vec![],
            sorting: None,
            limit: None,
        },
        view_config: json!({
            "lastView": "kanban",
            "kanban": { "groupBy": "spec_status" },
        }),
    }
}

/// Plans by approval state. The `draft` column is the approval queue: every
/// plan in it is holding its planned tasks at `open`.
fn plans_by_status_view() -> ViewStep {
    ViewStep {
        view_id: PLANS_BY_STATUS_ID,
        name: "Plans by Status",
        definition: QueryDefinition {
            target_type: "plan".to_string(),
            filters: vec![],
            sorting: None,
            limit: None,
        },
        view_config: json!({
            "lastView": "kanban",
            "kanban": { "groupBy": "plan_status" },
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::markdown::SeedTier;

    fn field_names(step: &SchemaStep) -> Vec<String> {
        step.params["fields"]
            .as_array()
            .expect("fields")
            .iter()
            .map(|f| f["name"].as_str().unwrap().to_string())
            .collect()
    }

    fn relationship<'a>(step: &'a SchemaStep, name: &str) -> &'a serde_json::Value {
        step.params["relationships"]
            .as_array()
            .expect("relationships")
            .iter()
            .find(|r| r["name"] == name)
            .unwrap_or_else(|| panic!("{} declares no `{name}`", step.schema_id))
    }

    /// Neither type may declare a bare `status`: it would be read as the core
    /// task lifecycle by every consumer that knows that name.
    #[test]
    fn spec_and_plan_carry_their_own_status_field_not_status() {
        let spec = field_names(&spec_schema());
        let plan = field_names(&plan_schema());
        assert!(!spec.contains(&"status".to_string()));
        assert!(!plan.contains(&"status".to_string()));
        for f in ["objective", "success_criteria", "boundaries", "spec_status"] {
            assert!(spec.contains(&f.to_string()), "spec missing {f}");
        }
        for f in ["approach", "risks", "plan_status"] {
            assert!(plan.contains(&f.to_string()), "plan missing {f}");
        }
    }

    /// The cardinalities the issue specifies, and the reverse names the gates
    /// walk (`node.plan`, `node.spec` on a task).
    #[test]
    fn lineage_relationships_have_the_specified_shape() {
        let (plan, spec) = (plan_schema(), spec_schema());

        let plan_spec = relationship(&plan, "spec");
        assert_eq!(plan_spec["targetType"], "spec");
        assert_eq!(plan_spec["cardinality"], "one");
        assert_eq!(plan_spec["reverseName"], "plans");
        assert_eq!(plan_spec["reverseCardinality"], "many");

        let plan_tasks = relationship(&plan, "tasks");
        assert_eq!(plan_tasks["targetType"], "task");
        assert_eq!(plan_tasks["cardinality"], "many");
        assert_eq!(plan_tasks["reverseName"], "plan");
        assert_eq!(plan_tasks["reverseCardinality"], "one");

        let spec_tasks = relationship(&spec, "tasks");
        assert_eq!(spec_tasks["targetType"], "task");
        assert_eq!(spec_tasks["cardinality"], "many");
        assert_eq!(spec_tasks["reverseName"], "spec");
        assert_eq!(spec_tasks["reverseCardinality"], "many");
    }

    /// ADR-063: a field added directly to a core type must be prefixed.
    #[test]
    fn the_task_field_is_namespaced() {
        let ext = verification_method_field();
        assert_eq!(ext.schema_id, "task");
        let name = ext.params["add_fields"][0]["name"].as_str().unwrap();
        assert!(name.starts_with("custom:"), "{name} must carry a prefix");
    }

    /// A reject on a reactive rule is refused at save time — there is no
    /// transaction left to veto.
    #[test]
    fn every_rule_is_an_invariant_reject() {
        for play in playbook().plays {
            for rule in play.rules.as_array().expect("rules") {
                assert_eq!(rule["class"], "invariant", "{}", rule["name"]);
                assert_eq!(rule["actions"][0]["action_type"], "reject");
            }
        }
    }

    /// The lock must never trigger on its own status field with a post-write
    /// check, or the move INTO superseded would be rejected.
    #[test]
    fn the_status_rule_of_the_lock_reads_the_old_value() {
        let play = supersession_lock();
        for rule in play.rules.as_array().unwrap() {
            let key = rule["trigger"]["property_key"].as_str().unwrap();
            if key.ends_with("_status") {
                assert!(
                    rule["conditions"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .any(|c| c.as_str().unwrap().contains("old_value")),
                    "{key}: a post-write-only check would reject superseding itself"
                );
            }
        }
    }

    #[test]
    fn play_ids_are_unique() {
        let pb = playbook();
        let mut ids: Vec<&str> = pb.plays.iter().map(|p| p.play_id).collect();
        ids.sort_unstable();
        let before = ids.len();
        ids.dedup();
        assert_eq!(before, ids.len());
    }

    /// SKILL.md recognises an installed Playbook by a skill titled
    /// `<name> Workspace`; a retitled overview silently breaks that routing.
    #[test]
    fn overview_is_titled_as_skill_md_routes_on() {
        let pb = playbook();
        let overview = crate::methodology::skills::playbook_overview_skill(
            pb.overview,
            &crate::methodology::skills::InstalledIds::default(),
        );
        assert_eq!(overview.title, format!("{} Workspace", pb.name));
        assert!(matches!(overview.tier, SeedTier::Starter));
    }

    #[test]
    fn skills_decode_and_are_starter_tier() {
        let skills = playbook().skills;
        assert_eq!(skills.len(), 4);
        for s in &skills {
            let skill = crate::models::SkillNode::from_properties(&s.title, &s.root_properties)
                .expect("playbook skill must decode");
            assert!(!skill.description.is_empty(), "{}", s.title);
            assert!(matches!(s.tier, SeedTier::Starter), "{}", s.title);
            assert!(s.child_node_type.is_none());
        }
    }
}
