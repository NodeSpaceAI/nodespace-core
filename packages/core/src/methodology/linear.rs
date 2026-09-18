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
use crate::methodology::{FieldValueExtension, MethodologyRecipe, PlayStep, SchemaStep};
use serde_json::json;

/// Days a cycle spans unless the user edits `cycle.duration_days`.
///
/// Two weeks is Linear's own default and the most common sprint length. It is
/// an ordinary schema default rather than a constant baked into the
/// cycle-creation Play, so changing it is a field edit, not a Play rewrite —
/// the Play reads `{item.duration_days}` off the preceding cycle.
const DEFAULT_CYCLE_DAYS: i64 = 14;

/// Cron for the two scheduled Plays: once daily at 00:05.
///
/// Both are date-boundary driven — "has the current cycle ended" is only ever
/// true at a day boundary — so a daily tick is the natural granularity, and a
/// few minutes after midnight avoids racing the boundary itself. The engine's
/// `CronRunner` wakes every 60s and evaluates against local wall clock, so
/// exact firing time is best-effort, not guaranteed.
const DAILY_AFTER_MIDNIGHT: &str = "0 5 0 * * * *";

/// The Linear-style recipe.
pub fn recipe() -> MethodologyRecipe {
    MethodologyRecipe {
        id: "linear",
        name: "Linear-style",
        description:
            "Issues with point estimates and a richer status vocabulary, time-boxed Cycles \
             with automatic creation and rollover, and validation gates that stop an issue \
             closing with open sub-issues or starting with an unresolved blocker.",
        schemas: vec![issue_schema(), cycle_schema()],
        field_value_extensions: vec![issue_status_values(), issue_priority_values()],
        plays: vec![
            cycle_creation_play(),
            cycle_rollover_play(),
            sub_issue_completion_gate(),
            blocker_gate(),
        ],
        skills: vec![creating_an_issue(), working_with_cycles(), validation_rules()],
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
                         the rollover Play moves its unfinished work forward.",
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

/// Create the next cycle before the current one ends.
///
/// Scans cycles daily; when one ends today, creates its successor starting
/// tomorrow and running for that cycle's own `duration_days`. Both dates come
/// from `add_days` (#2640) — neither CEL nor action-value resolution can
/// compute a date otherwise.
///
/// Derived identity (ADR-074) makes this safe on several devices at once: the
/// created node's id is a function of `(rule_id, action_index, [scanned cycle
/// id])`, so every device computes the same id for the same successor and the
/// writes collapse to one row rather than N siblings.
fn cycle_creation_play() -> PlayStep {
    PlayStep {
        play_id: "linear-cycle-creation",
        name: "Create the next cycle",
        description:
            "When a cycle reaches its end date, create its successor starting the next day, \
             spanning that cycle's own duration_days.",
        rules: json!([{
            "name": "create-successor-cycle",
            "trigger": {
                "type": "scheduled",
                "cron": DAILY_AFTER_MIDNIGHT,
                "node_type": "cycle",
            },
            "conditions": [
                "node.end_date == today()",
            ],
            "actions": [{
                "action_type": "create_node",
                "params": {
                    "node_type": "cycle",
                    "content": "Next cycle",
                    "properties": {
                        "start_date": "{add_days(trigger.node.end_date, 1)}",
                        "end_date": "{add_days(trigger.node.end_date, trigger.node.duration_days)}",
                        "duration_days": "{trigger.node.duration_days}",
                    },
                },
            }],
        }]),
    }
}

/// Move unfinished work out of an ended cycle.
///
/// Runs the day after a cycle ends, so the successor the creation Play makes
/// on the end date already exists. Reassignment is an `add_relationship` onto
/// the successor; the old edge is left in place as history, matching how
/// Linear shows which cycle an issue slipped from.
fn cycle_rollover_play() -> PlayStep {
    PlayStep {
        play_id: "linear-cycle-rollover",
        name: "Roll unfinished work into the next cycle",
        description:
            "The day after a cycle ends, move each of its unfinished tasks to the successor \
             cycle. Work that is done or cancelled stays where it was.",
        rules: json!([{
            "name": "rollover-incomplete-tasks",
            "trigger": {
                "type": "scheduled",
                "cron": DAILY_AFTER_MIDNIGHT,
                "node_type": "cycle",
            },
            "conditions": [
                "node.end_date == add_days(today(), -1)",
            ],
            "actions": [{
                "action_type": "add_relationship",
                "for_each": "node.tasks",
                "params": {
                    "source_id": "{trigger.node.id}",
                    "relationship_type": "tasks",
                    "target_id": "{item.id}",
                },
            }],
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
                "property_key": "status",
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
                "property_key": "status",
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
// than one broad "Linear methodology" skill. ADR-038's retrieval drops a
// skill scoring below 0.8 similarity, which rewards precise matches and
// penalizes a single skill diluted across several intents.
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

**Rollover.** The day after a cycle ends, unfinished work is moved to the
successor. Done and cancelled work stays put. The old edge is kept, so an issue
that slipped shows which cycle it came from.

Both run daily just after midnight, on whichever devices are online. If several
are, they converge on the same result rather than duplicating it.

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
        for play in recipe().plays {
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
        for play in recipe().plays {
            for rule in play.rules.as_array().expect("rules array") {
                if rule["trigger"]["type"] == "scheduled" {
                    assert!(rule["trigger"]["cron"].is_string());
                    assert!(rule["trigger"]["node_type"].is_string());
                }
            }
        }
    }

    #[test]
    fn play_ids_are_unique() {
        let r = recipe();
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
        let skills = recipe().skills;
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

    /// Recipe content is opt-in, so it must not be seeded at startup with the
    /// System-tier content.
    #[test]
    fn recipe_skills_are_starter_tier() {
        for s in recipe().skills {
            assert!(matches!(s.tier, SeedTier::Starter), "{}", s.title);
        }
    }
}
