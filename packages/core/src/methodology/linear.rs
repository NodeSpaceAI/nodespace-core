//! Linear-style work tracking: Issues, Cycles, and the automation around them.
//!
//! Models the parts of Linear's workflow that are genuinely structural — a
//! richer status vocabulary, point estimates, time-boxed cycles with
//! rollover, and two hard validation gates — using mechanisms NodeSpace
//! already has. Nothing here is Linear-specific platform code; it is a
//! composition of `extends`, `add_field_values`, relationships and Plays.
//!
//! # `issue` is a real type, not a relabelled task
//!
//! `issue` declares `extends: task` (ADR-078), so an instance carries
//! `node_type: "issue"` and its own `estimate` field, while inheriting every
//! `task` field and relationship. Subtype-aware querying means a query or
//! Play written against `task` still matches issues, with no rewrite.
//!
//! # The status vocabulary extends the inherited field
//!
//! `backlog`, `triage` and `in_review` are appended to the *inherited*
//! `task.status` — not a separate `issue_status` field. Keeping the field
//! name identical is what lets `task`-scoped consumers keep matching. Each
//! appended value carries `mapsTo` naming the base value it collapses to at
//! `task` scope, so a Play or query reading at `task` scope sees `open` where
//! an issue stores `backlog`.
//!
//! Note the base vocabulary is `open`/`in_progress`/`done`/`cancelled` —
//! there is no `todo` value in NodeSpace, so `backlog` and `triage` both map
//! to `open`.

use crate::markdown::{NodeTemplate, SeedTier};
use crate::methodology::{FieldValueExtension, MethodologyPlaybook, PlayStep, SchemaStep};
use serde_json::json;

/// Days a cycle spans unless the user edits `cycle.duration_days`.
///
/// Two weeks is Linear's own default and the most common sprint length. It is
/// an ordinary schema default rather than a constant baked into the
/// rollover Play, so changing it is a field edit, not a Play rewrite —
/// the Play reads `{trigger.node.duration_days}` off the ending cycle.
const DEFAULT_CYCLE_DAYS: i64 = 14;

/// Cron for the cycle-rollover Play: once daily at 00:05.
///
/// It is date-boundary driven — "has the current cycle ended" is only ever
/// true at a day boundary — so a daily tick is the natural granularity, and a
/// few minutes after midnight avoids racing the boundary itself. The engine's
/// `CronRunner` wakes every 60s and evaluates against local wall clock, so
/// exact firing time is best-effort, not guaranteed.
const DAILY_AFTER_MIDNIGHT: &str = "0 5 0 * * * *";

/// The Linear-style playbook.
pub fn playbook() -> MethodologyPlaybook {
    MethodologyPlaybook {
        id: "linear",
        name: "Linear-style",
        description:
            "Issues with point estimates and a richer status vocabulary, time-boxed Cycles \
             with automatic creation and rollover, and validation gates that stop an issue \
             closing with open sub-issues or starting with an unresolved blocker.",
        schemas: vec![issue_schema(), cycle_schema()],
        field_value_extensions: vec![issue_status_values(), issue_priority_values()],
        plays: vec![
            cycle_rollover_play(),
            sub_issue_completion_gate(),
            blocker_gate(),
        ],
        skills: vec![
            creating_an_issue(),
            working_with_cycles(),
            validation_rules(),
        ],
    }
}

// ---------------------------------------------------------------------------
// Schemas
// ---------------------------------------------------------------------------

/// `issue` — a `task` specialized with a point estimate.
///
/// Declares exactly one field of its own. Everything else Linear's Issue has
/// that NodeSpace models — status, priority, assignee, blocking and relation
/// edges, sub-issues via `has_child`, labels via Collections — is inherited
/// or already universal, so the schema stays this small deliberately.
fn issue_schema() -> SchemaStep {
    SchemaStep {
        schema_id: "issue",
        params: json!({
            "name": "Issue",
            "extends": "task",
            "description":
                "A unit of work in a Linear-style workflow. Extends task with a point \
                 estimate and a richer status vocabulary. Sub-issues are ordinary child \
                 nodes; labels and teams are Collections; blocking and relation edges are \
                 inherited from task.",
            "fields": [
                {
                    "name": "estimate",
                    "friendlyName": "Estimate",
                    "type": "enum",
                    "protection": "user",
                    "indexed": true,
                    "required": false,
                    "extensible": true,
                    "coreValues": [
                        { "value": "1", "label": "1 point" },
                        { "value": "2", "label": "2 points" },
                        { "value": "3", "label": "3 points" },
                        { "value": "5", "label": "5 points" },
                        { "value": "8", "label": "8 points" },
                    ],
                    "userValues": [],
                    "description":
                        "Relative size in points, on the modified-Fibonacci scale Linear \
                         uses. Points rather than hours: the scale is deliberately coarse \
                         and gappy at the top so large items are estimated as clearly \
                         large rather than precisely wrong. Extensible, so a team wanting \
                         13 or 21 can add them.",
                },
            ],
        }),
    }
}

/// `cycle` — a time-boxed iteration owning a set of tasks.
///
/// Has no stored `status`. A cycle's upcoming/active/past state is entirely
/// derivable from `start_date`/`end_date` against today, and storing it would
/// require a second Play whose only job was keeping the stored value honest.
/// This is the opposite of core `project`, which correctly does store a
/// status — Linear's Project has a real one that is not a function of dates.
fn cycle_schema() -> SchemaStep {
    SchemaStep {
        schema_id: "cycle",
        params: json!({
            "name": "Cycle",
            "description":
                "A time-boxed iteration. Its upcoming/active/past state is derived by \
                 comparing start_date and end_date to today — deliberately not stored, so \
                 there is no second copy of the truth to keep in sync.",
            "fields": [
                {
                    "name": "start_date",
                    "friendlyName": "Start date",
                    "type": "date",
                    "protection": "user",
                    "indexed": true,
                    "required": true,
                    "description": "First day of the cycle.",
                },
                {
                    "name": "end_date",
                    "friendlyName": "End date",
                    "type": "date",
                    "protection": "user",
                    "indexed": true,
                    "required": true,
                    "description":
                        "Last day of the cycle. A cycle whose end_date has passed is over; \
                         on that day the rollover Play moves its tasks into the successor.",
                },
                {
                    "name": "duration_days",
                    "friendlyName": "Duration (days)",
                    "type": "number",
                    "protection": "user",
                    "indexed": false,
                    "required": false,
                    "default": DEFAULT_CYCLE_DAYS,
                    "description":
                        "How many days the NEXT cycle should span. Read by the \
                         cycle-creation Play when it computes the successor's end_date, so \
                         changing cadence is a field edit rather than a Play rewrite.",
                },
            ],
            "relationships": [
                {
                    "name": "tasks",
                    "targetType": "task",
                    "direction": "out",
                    "cardinality": "many",
                    // Shares a spelling with the `cycle` schema id. Safe
                    // because the install's schema rewrite is key-targeted
                    // (`extends`/`targetType` only) rather than a blanket
                    // value walk — a reverse NAME is vocabulary, not a
                    // reference, and must survive a re-key untouched.
                    "reverseName": "cycle",
                    "reverseCardinality": "one",
                    "description":
                        "Work assigned to this cycle. Targets `task`, not `issue`: \
                         subtype-aware querying already reaches issues through it, and \
                         targeting the base type keeps a plain task assignable to a cycle.",
                },
            ],
        }),
    }
}

// ---------------------------------------------------------------------------
// Vocabulary extensions
// ---------------------------------------------------------------------------

/// Append Linear's extra statuses to the inherited `task.status`.
///
/// Every value needs `mapsTo` because the field is inherited: a `task`-scoped
/// reader must see a value it understands. `backlog` and `triage` both
/// collapse to `open` — the base vocabulary has no `todo`.
fn issue_status_values() -> FieldValueExtension {
    FieldValueExtension {
        schema_id: "issue",
        field: "status",
        params: json!({
            "schema_id": "issue",
            "add_field_values": [
                {
                    "field": "status",
                    "values": [
                        { "value": "triage", "label": "Triage", "mapsTo": "open" },
                        { "value": "backlog", "label": "Backlog", "mapsTo": "open" },
                        { "value": "in_review", "label": "In Review", "mapsTo": "in_progress" },
                    ],
                },
            ],
        }),
    }
}

/// Append `urgent` and `none` to the inherited `task.priority`.
///
/// No `mapsTo` values here, unlike status: priority has no category model to
/// preserve. It is a flat scale, and these extend it at both ends — `urgent`
/// above `highest`, `none` as an explicit "deliberately unprioritized" that
/// is distinct from the field simply being unset.
fn issue_priority_values() -> FieldValueExtension {
    FieldValueExtension {
        schema_id: "issue",
        field: "priority",
        params: json!({
            "schema_id": "issue",
            "add_field_values": [
                {
                    "field": "priority",
                    "values": [
                        { "value": "urgent", "label": "Urgent", "mapsTo": "highest" },
                        { "value": "none", "label": "No priority", "mapsTo": "lowest" },
                    ],
                },
            ],
        }),
    }
}

// ---------------------------------------------------------------------------
// Plays
// ---------------------------------------------------------------------------

/// Close out an ending cycle: create its successor, then move its tasks into
/// it.
///
/// One Play with two actions rather than two Plays, because the second action
/// needs the first's output. The successor's id is only reachable as
/// `{actions[0].result.id}` — nothing in the binding context can name "the
/// cycle starting tomorrow", so a separate rollover Play has no way to
/// address the node it is supposed to move work into.
///
/// Running both on the end date also removes a cross-day dependency: a
/// rollover that fired the next morning would be assuming the creation Play
/// had already succeeded, and would silently do nothing if it had not.
///
/// The reassignment is add-then-remove, and both halves are required. Adding
/// alone does not move anything: only the FORWARD cardinality is enforced on
/// write, `cycle.tasks` is `many` on that side, and the idempotency check is
/// keyed on `(source, target, name)` — so a second cycle claiming the same
/// task is accepted rather than rejected or replaced. Without the removal a
/// task accumulates one edge per cycle forever and every `sum(cycle.tasks,
/// estimate)` double-counts.
///
/// Add before remove, deliberately: a failure between the two leaves the task
/// in both cycles, which is visible and repairable, rather than in neither,
/// which silently loses it.
///
/// **Every task moves, including finished ones.** `for_each` has no per-item
/// filter — `ActionDefinition` carries only `action_type`, `params` and
/// `for_each`, and rule conditions compile once against the trigger node, so
/// nothing can see `item`. Filtering to incomplete work needs a capability
/// the engine does not have. This is stated plainly in the Play's own
/// description and in the seeded guidance rather than described as intent,
/// because an agent reading either will act on it.
///
/// Both dates come from `add_days`; neither CEL nor action-value resolution
/// can otherwise compute one.
///
/// Derived identity (ADR-074) makes this safe on several devices at once. The
/// created cycle's id is a function of `(rule_id, action_index, [ending cycle
/// id])`, so every device computes the same id for the same successor and the
/// writes collapse to one row. The `for_each` extends that path with each
/// task's own id, so the reassignments collapse the same way.
fn cycle_rollover_play() -> PlayStep {
    PlayStep {
        play_id: "linear-cycle-rollover",
        name: "Close out the ending cycle",
        description: "On the day a cycle ends, create its successor — starting the next day and \
             spanning that cycle's own duration_days — then move the ending cycle's tasks \
             into it. Every task moves, finished ones included: the engine has no per-item \
             filter for a for_each yet.",
        rules: json!([{
            "name": "create-successor-and-roll-over",
            "trigger": {
                "type": "scheduled",
                "cron": DAILY_AFTER_MIDNIGHT,
                "node_type": "cycle",
            },
            "conditions": [
                "node.end_date == today()",
            ],
            "actions": [
                {
                    "action_type": "create_node",
                    "params": {
                        "node_type": "cycle",
                        "content": "Next cycle",
                        "properties": {
                            "start_date": "{add_days(trigger.node.end_date, 1)}",
                            "end_date":
                                "{add_days(trigger.node.end_date, trigger.node.duration_days)}",
                            "duration_days": "{trigger.node.duration_days}",
                        },
                    },
                },
                {
                    "action_type": "add_relationship",
                    "for_each": "trigger.node.tasks",
                    "params": {
                        "source_id": "{actions[0].result.id}",
                        "relationship_type": "tasks",
                        "target_id": "{item.id}",
                    },
                },
                {
                    "action_type": "remove_relationship",
                    "for_each": "trigger.node.tasks",
                    "params": {
                        "source_id": "{trigger.node.id}",
                        "relationship_type": "tasks",
                        "target_id": "{item.id}",
                    },
                },
            ],
        }]),
    }
}

/// Refuse to close an issue that still has open sub-issues.
///
/// Invariant class, so it runs synchronously inside the triggering
/// transaction and can veto the write (#2642/#2643). A reactive rule could
/// only complain after the fact.
///
/// Sub-issues are ordinary `has_child` children — no schema models them, and
/// this gate works for any nesting the outline allows.
fn sub_issue_completion_gate() -> PlayStep {
    PlayStep {
        play_id: "linear-sub-issue-gate",
        name: "Block closing an issue with open sub-issues",
        description:
            "Rejects a status change to done while any child issue is still open. Close the \
             children first, or move them out from under this issue.",
        rules: json!([{
            "name": "reject-done-with-open-children",
            "class": "invariant",
            "trigger": {
                "type": "graph_event",
                "on": "property_changed",
                "node_type": "issue",
                // Namespaced, not bare: `update_node` stores a schema field
                // under the node's own type object and reports the change as
                // `"{node_type}.{field}"`, so that is what a trigger's
                // `property_key` matches against. A bare "status" matches
                // nothing and the rule silently never fires.
                "property_key": "issue.status",
            },
            "conditions": [
                "node.status == 'done'",
                "node.has_child.exists(c, c.status != 'done' && c.status != 'cancelled')",
            ],
            "actions": [{
                "action_type": "reject",
                "params": {
                    "message":
                        "This issue still has open sub-issues. Close or cancel them first, \
                         or move them out from under this issue.",
                },
            }],
        }]),
    }
}

/// Refuse to start an issue whose blockers are unresolved.
///
/// Uses the `blocked_by` reverse edge (#2637 declares the pair on `task`;
/// #2641 made reverse traversal resolvable in conditions). Fires on the
/// transition into `in_progress` specifically — an issue may sit in `backlog`
/// or `triage` behind a blocker quite legitimately.
fn blocker_gate() -> PlayStep {
    PlayStep {
        play_id: "linear-blocker-gate",
        name: "Block starting an issue with an open blocker",
        description:
            "Rejects a status change to in_progress while anything blocking this issue is \
             still open. Resolve the blocker, or drop the blocks edge if it no longer applies.",
        rules: json!([{
            "name": "reject-start-with-open-blocker",
            "class": "invariant",
            "trigger": {
                "type": "graph_event",
                "on": "property_changed",
                "node_type": "issue",
                // Namespaced, not bare: `update_node` stores a schema field
                // under the node's own type object and reports the change as
                // `"{node_type}.{field}"`, so that is what a trigger's
                // `property_key` matches against. A bare "status" matches
                // nothing and the rule silently never fires.
                "property_key": "issue.status",
            },
            "conditions": [
                "node.status == 'in_progress'",
                "node.blocked_by.exists(b, b.status != 'done' && b.status != 'cancelled')",
            ],
            "actions": [{
                "action_type": "reject",
                "params": {
                    "message":
                        "This issue is blocked by work that is not finished yet. Resolve the \
                         blocker first, or remove the blocks relationship if it no longer applies.",
                },
            }],
        }]),
    }
}

// ---------------------------------------------------------------------------
// Seeded skills
// ---------------------------------------------------------------------------

// Narrow and task-scoped, mirroring the built-in skills' own shape rather
// than one broad "Linear methodology" skill.
//
// Retrieval (`skill_ops::find_skills`) is pure KNN cosine over skill ROOTS,
// limit-capped, with the threshold at 0.0 — the cosine noise floor, not a
// confidence cutoff (ADR-038 describes an 0.8 floor; the code deliberately
// moved past that so the model judges confidence from the raw score). Two
// consequences for anything seeded here:
//
//   - The markdown body is NOT indexed. Only the root's title and
//     `description` are, so the description is the entire retrieval surface.
//   - Nothing is filtered out; a weak description is out-RANKED. These
//     compete directly with the 11 built-ins, so "Creating an Issue" loses to
//     "Node Creation" on a query like "file a bug" unless its description is
//     written in the words a request actually arrives in.
//
// These carry the cross-schema narrative no per-schema description can: how
// an Issue relates to a Cycle, what rollover does, why a write was rejected.

fn skill(title: &str, description: &str, markdown: &str) -> NodeTemplate {
    NodeTemplate {
        title: title.to_string(),
        content: None,
        markdown_content: markdown.to_string(),
        root_node_type: "skill".to_string(),
        root_properties: serde_json::json!({
            "description": description,
            "tool_whitelist": ["create_node", "update_node", "search_nodes", "get_node"],
            "max_iterations": 3,
        }),
        child_node_type: None,
        child_properties: None,
        tier: SeedTier::Starter,
    }
}

fn creating_an_issue() -> NodeTemplate {
    skill(
        "Creating an Issue",
        "How to create an issue in a Linear-style workspace: when to use issue rather than \
         task, the extended status and priority vocabularies, and point estimates.",
        r#"# Creating an Issue

`issue` extends `task`. Create an `issue` for tracked product or engineering
work; a plain `task` is still right for a one-off to-do that is not part of the
tracked workflow.

An issue carries everything a task does — assignee, due date, blocking edges —
plus the following.

## Status

`status` is `task`'s own field with extra values, not a separate field:

| Value | Means | Reads as, to a task-scoped reader |
|---|---|---|
| `triage` | Not yet assessed | `open` |
| `backlog` | Assessed, not scheduled | `open` |
| `open` | Ready to pick up | `open` |
| `in_progress` | Being worked | `in_progress` |
| `in_review` | Work done, awaiting review | `in_progress` |
| `done` | Complete | `done` |
| `cancelled` | Abandoned | `cancelled` |

The right-hand column matters: a query or Play written against `task` sees the
mapped value, never the raw one. So a `task`-scoped report counting
`in_progress` work includes issues sitting in `in_review`.

## Priority

`urgent` sits above `highest`; `none` means deliberately unprioritized, which
is different from leaving the field unset.

## Estimate

Points on a modified-Fibonacci scale: 1, 2, 3, 5, 8. The gaps are the point —
an 8 says "clearly large" rather than a precise wrong number. Leave it unset
rather than guessing; the field is extensible if a team wants 13 or 21.

## Sub-issues

A sub-issue is just an issue nested under another in the outline. There is no
field for it, and no special call — create the child and put it under the
parent. Note the completion gate: a parent cannot be closed while a child is
still open.

## Labels and teams

Collections, not fields. A label is a collection of issues; a team is a
collection of people. Add membership rather than looking for a `labels` field.
"#,
    )
}

fn working_with_cycles() -> NodeTemplate {
    skill(
        "Working with Cycles",
        "How cycles work in a Linear-style workspace: assigning work via the tasks \
         relationship, the derived active/past state, and what the automatic cycle-creation \
         and rollover Plays do.",
        r#"# Working with Cycles

A `cycle` is a time-boxed iteration — Linear's sprint equivalent.

## Assigning work

Work joins a cycle through the `tasks` relationship, not a property. Create the
edge from the cycle to the task or issue; there is no `cycle_id` field to set.

The relationship targets `task`, so plain tasks and issues can both be assigned.
Reading `cycle.tasks` returns both.

## There is no status field

A cycle's state is derived by comparing its dates to today:

- `start_date` in the future → upcoming
- today between the dates → active
- `end_date` in the past → over

Do not look for a `status` property and do not add one. Storing it would mean
two copies of the same truth, one of which would drift.

## What runs automatically

**Cycle creation.** On the day a cycle ends, its successor is created, starting
the next day and running for `duration_days` — read off the ending cycle, so
changing cadence means editing that field, not the Play.

**Create the first cycle yourself.** This triggers on an existing cycle reaching
its end date, so with no cycle in the graph nothing ever fires. Create one with
a `start_date` and `end_date`; the automation takes over from there.

**Rollover.** In the same run, the ending cycle's tasks move to the successor —
added to the new cycle and removed from the old one, so a task belongs to
exactly one cycle.

Every task moves, completed ones included. That is a current limitation rather
than a design choice: a `for_each` action cannot filter per item yet. If a
finished cycle should keep its completed work, move those tasks back afterwards.

Runs daily just after midnight, on whichever devices are online. If several are,
they converge on the same result rather than duplicating it.

## Estimate totals

Not stored. Sum `estimate` across `cycle.tasks` when a total is wanted, rather
than expecting a field.
"#,
    )
}

fn validation_rules() -> NodeTemplate {
    skill(
        "Issue Validation Rules",
        "Why a status change on an issue was rejected: the sub-issue completion gate and the \
         blocker gate, what each checks, and how to proceed when one fires.",
        r#"# Issue Validation Rules

Two rules can reject a status change outright. A rejection is the system
working as configured — not a bug, and not something to retry unchanged.

Both run synchronously, inside the transaction of the write they are checking,
so a rejected change never partially lands.

## Cannot close with open sub-issues

Setting `status` to `done` is rejected while any child issue is not `done` or
`cancelled`.

To proceed, either close or cancel the children, or move them out from under
this issue if they do not really belong to it.

## Cannot start with an open blocker

Setting `status` to `in_progress` is rejected while anything on `blocked_by` is
not `done` or `cancelled`.

To proceed, either finish the blocker, or remove the `blocks` edge if it no
longer applies. Note this gate is specific to starting work — an issue can sit
in `triage` or `backlog` behind a blocker quite legitimately.

## If a rejection looks wrong

Report it rather than working around it. Both rules are ordinary Plays the user
can inspect, edit or disable, and both were installed as part of the methodology
setup — so a rejection that seems incorrect is a question about their
configuration, not something to route around by, say, writing the status through
a different path.
"#,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn issue_extends_task_and_declares_only_estimate() {
        let s = issue_schema();
        assert_eq!(s.schema_id, "issue");
        assert_eq!(s.params["extends"], "task");

        let fields = s.params["fields"].as_array().expect("fields array");
        assert_eq!(
            fields.len(),
            1,
            "issue owns exactly one field; everything else is inherited"
        );
        assert_eq!(fields[0]["name"], "estimate");
    }

    /// The base vocabulary is open/in_progress/done/cancelled — there is no
    /// `todo`. `add_field_values` rejects a `mapsTo` naming a value the field
    /// does not already have, so a mapping to `todo` would fail at install.
    #[test]
    fn every_extended_status_maps_to_a_real_base_value() {
        const BASE: [&str; 4] = ["open", "in_progress", "done", "cancelled"];

        let ext = issue_status_values();
        let values = ext.params["add_field_values"][0]["values"]
            .as_array()
            .expect("values array");

        assert!(!values.is_empty());
        for v in values {
            let maps_to = v["mapsTo"]
                .as_str()
                .unwrap_or_else(|| panic!("{} must declare mapsTo", v["value"]));
            assert!(
                BASE.contains(&maps_to),
                "{} maps to '{}', which is not a base task.status value",
                v["value"],
                maps_to
            );
        }
    }

    #[test]
    fn extended_priorities_map_to_real_base_values() {
        const BASE: [&str; 5] = ["highest", "high", "medium", "low", "lowest"];

        let ext = issue_priority_values();
        let values = ext.params["add_field_values"][0]["values"]
            .as_array()
            .expect("values array");

        for v in values {
            let maps_to = v["mapsTo"].as_str().expect("mapsTo");
            assert!(
                BASE.contains(&maps_to),
                "{} maps to '{}', not a base task.priority value",
                v["value"],
                maps_to
            );
        }
    }

    /// A cycle's state is derived from its dates. A stored `status` would be
    /// a second copy of that truth, needing its own Play to stay honest.
    #[test]
    fn cycle_has_no_status_field() {
        let fields = cycle_schema().params["fields"]
            .as_array()
            .expect("fields")
            .iter()
            .map(|f| f["name"].as_str().unwrap().to_string())
            .collect::<Vec<_>>();

        assert!(!fields.contains(&"status".to_string()));
        for expected in ["start_date", "end_date", "duration_days"] {
            assert!(fields.contains(&expected.to_string()), "missing {expected}");
        }
    }

    /// Mirrors `project.tasks`: out/many with a one-cardinality reverse, and
    /// targeting `task` so subtype-aware querying reaches issues for free.
    #[test]
    fn cycle_declares_a_tasks_relationship_targeting_task() {
        let rels = cycle_schema().params["relationships"]
            .as_array()
            .expect("relationships")
            .clone();
        assert_eq!(rels.len(), 1);

        let r = &rels[0];
        assert_eq!(r["name"], "tasks");
        assert_eq!(r["targetType"], "task");
        assert_eq!(r["direction"], "out");
        assert_eq!(r["cardinality"], "many");
        assert_eq!(r["reverseName"], "cycle");
        assert_eq!(r["reverseCardinality"], "one");
    }

    /// A `reject` action on a reactive rule is refused by save-time
    /// validation: once a rule runs post-commit there is no transaction left
    /// to veto. Both gates must therefore declare `class: invariant`.
    #[test]
    fn rules_using_reject_are_declared_invariant() {
        for play in playbook().plays {
            for rule in play.rules.as_array().expect("rules array") {
                let uses_reject = rule["actions"]
                    .as_array()
                    .map(|a| a.iter().any(|x| x["action_type"] == "reject"))
                    .unwrap_or(false);
                if uses_reject {
                    assert_eq!(
                        rule["class"], "invariant",
                        "rule '{}' in {} uses reject and must be invariant",
                        rule["name"], play.play_id
                    );
                }
            }
        }
    }

    #[test]
    fn scheduled_rules_declare_a_cron_and_a_node_type() {
        for play in playbook().plays {
            for rule in play.rules.as_array().expect("rules array") {
                if rule["trigger"]["type"] == "scheduled" {
                    assert!(rule["trigger"]["cron"].is_string());
                    assert!(rule["trigger"]["node_type"].is_string());
                }
            }
        }
    }

    /// Rollover must move work into the cycle the same rule just created, not
    /// back into the one that is ending.
    ///
    /// Worth pinning explicitly because the wrong version installs perfectly
    /// happily: `{trigger.node.id}` is a valid binding that resolves to a real
    /// cycle, so every structural check passes and the Play simply re-adds
    /// each task to the cycle it is already in. Nothing fails; the work just
    /// never moves.
    ///
    /// This asserts SHAPE only, which is exactly its limitation — it cannot
    /// see whether the actions achieve anything. `rollover_moves_a_task_...`
    /// in `tests/methodology_linear_execution_test.rs` runs them and counts
    /// the edges, and is what actually pins the behavior.
    #[test]
    fn rollover_adds_to_the_successor_then_removes_from_the_ending_cycle() {
        let play = cycle_rollover_play();
        let rules = play.rules.as_array().expect("rules");
        let actions = rules[0]["actions"].as_array().expect("actions");

        assert_eq!(
            actions[0]["action_type"], "create_node",
            "the successor must be created before anything can be moved into it"
        );
        assert_eq!(actions[0]["params"]["node_type"], "cycle");

        let add = &actions[1];
        assert_eq!(add["action_type"], "add_relationship");
        assert_eq!(
            add["params"]["source_id"], "{actions[0].result.id}",
            "must target the created successor, not the ending cycle"
        );
        assert_eq!(add["params"]["target_id"], "{item.id}");

        // Adding alone is not a move: only forward cardinality is enforced on
        // write, and `tasks` is `many` there, so the old edge survives unless
        // something removes it.
        let remove = actions
            .get(2)
            .expect("a third action must remove the old edge, or the task ends up in both cycles");
        assert_eq!(remove["action_type"], "remove_relationship");
        assert_eq!(
            remove["params"]["source_id"], "{trigger.node.id}",
            "the removal must target the ENDING cycle"
        );
        assert_eq!(remove["params"]["target_id"], "{item.id}");

        // Add before remove: a failure between them leaves the task in both
        // cycles (visible, repairable) rather than in neither (silently lost).
        assert!(
            actions
                .iter()
                .position(|a| a["action_type"] == "add_relationship")
                < actions
                    .iter()
                    .position(|a| a["action_type"] == "remove_relationship"),
            "add must precede remove"
        );

        // `node.*` is condition syntax; action bindings only know
        // `trigger.node.*`, `item.*` and `actions[N].*`. A bare `node.tasks`
        // fails at runtime with `unknown binding root: 'node'`.
        for action in [add, remove] {
            assert_eq!(
                action["for_each"], "trigger.node.tasks",
                "for_each must use an action-binding root"
            );
        }
    }

    #[test]
    fn play_ids_are_unique() {
        let r = playbook();
        let mut ids: Vec<&str> = r.plays.iter().map(|p| p.play_id).collect();
        ids.sort_unstable();
        let before = ids.len();
        ids.dedup();
        assert_eq!(before, ids.len(), "play ids must be unique");
    }

    /// Narrow and task-scoped, per ADR-038's similarity threshold — one broad
    /// skill would score below it for every specific intent.
    #[test]
    fn skills_are_narrow_and_carry_guidance() {
        let skills = playbook().skills;
        assert!(skills.len() >= 3, "expected several narrow skills");

        for s in &skills {
            assert_eq!(s.root_node_type, "skill");
            assert!(
                s.root_properties["description"]
                    .as_str()
                    .is_some_and(|d| !d.is_empty()),
                "{} needs a description for retrieval",
                s.title
            );
            assert!(
                s.markdown_content.contains('#'),
                "{} should carry structured guidance",
                s.title
            );
            // ADR-057: guidance children are ordinary markdown nodes.
            assert!(s.child_node_type.is_none());
        }
    }

    /// Playbook content is opt-in, so it must not be seeded at startup with the
    /// System-tier content.
    #[test]
    fn playbook_skills_are_starter_tier() {
        for s in playbook().skills {
            assert!(matches!(s.tier, SeedTier::Starter), "{}", s.title);
        }
    }
}
