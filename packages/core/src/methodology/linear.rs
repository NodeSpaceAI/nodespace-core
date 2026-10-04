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

use crate::methodology::skills::{PlaybookSkill, DEFAULT_TOOLS};
use crate::methodology::{
    view_filters, FieldValueExtension, MethodologyPlaybook, PlayStep, SchemaStep, ViewStep,
};
use crate::services::QueryDefinition;
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

// The fixed ids of the nodes this playbook seeds. A seeded node's identity
// is its id (ADR-086 §10), and ADR-060 §5 orders rules across plays by play
// id, so each is a literal UUID rather than one minted per install.
/// The `linear-cycle-rollover` play.
pub const CYCLE_ROLLOVER_ID: &str = "c0e62d2e-3f5b-4f0a-9d36-1b7f1e6a4c01";
/// The `linear-sub-issue-gate` play.
pub const SUB_ISSUE_GATE_ID: &str = "c0e62d2e-3f5b-4f0a-9d36-1b7f1e6a4c02";
/// The `linear-blocker-gate` play.
pub const BLOCKER_GATE_ID: &str = "c0e62d2e-3f5b-4f0a-9d36-1b7f1e6a4c03";
/// The `linear-issues-by-status` saved view.
pub const ISSUES_BY_STATUS_ID: &str = "c0e62d2e-3f5b-4f0a-9d36-1b7f1e6a4c04";
/// The `linear-current-cycle-issues` saved view.
pub const CURRENT_CYCLE_ISSUES_ID: &str = "c0e62d2e-3f5b-4f0a-9d36-1b7f1e6a4c05";

/// The guidance skills, in install order.
const SKILLS: &[PlaybookSkill] = &[
    PlaybookSkill {
        // Creating an Issue
        id: "c0e62d2e-3f5b-4f0a-9d36-1b7f1e6a4c06",
        title: "Creating an Issue",
        description: "Report a bug, defect, crash or something broken, open a ticket, or raise \
                      an issue. Use when the user wants to file or log a bug, open a ticket, \
                      report a problem, or request a feature.",
        exclusion: Some("Add a task or a reminder."),
        tools: DEFAULT_TOOLS,
        applies_to: &["issue"],
        body: include_str!("skills/linear/creating-an-issue.md"),
    },
    PlaybookSkill {
        // Sprints and Cycles
        id: "c0e62d2e-3f5b-4f0a-9d36-1b7f1e6a4c07",
        title: "Sprints and Cycles",
        description: "Start, plan or close out a sprint or cycle, put issues in the current \
                      sprint, roll unfinished issues into the next sprint, and total a sprint's \
                      points. Use when the user says start the sprint, what's in this cycle, or \
                      how many points are in the sprint.",
        exclusion: Some("Add a task or a reminder."),
        tools: DEFAULT_TOOLS,
        applies_to: &["cycle", "issue"],
        body: include_str!("skills/linear/sprints-and-cycles.md"),
    },
    PlaybookSkill {
        // Issue Validation Rules
        id: "c0e62d2e-3f5b-4f0a-9d36-1b7f1e6a4c08",
        title: "Issue Validation Rules",
        description: "Why an issue won't close or won't start: its status change was rejected \
                      because sub-issues are still open or a blocker isn't done. Use when the \
                      user says it won't let me mark this done, it won't let me move this to in \
                      progress, why can't I close this, or why is this blocked.",
        exclusion: Some("Link a task to a decision."),
        tools: DEFAULT_TOOLS,
        applies_to: &["issue"],
        body: include_str!("skills/linear/issue-validation-rules.md"),
    },
];

/// The bundle-level skill.
const OVERVIEW: PlaybookSkill = PlaybookSkill {
    // Linear-style Workspace
    id: "c0e62d2e-3f5b-4f0a-9d36-1b7f1e6a4c09",
    title: "Linear-style Workspace",
    description: "What workflow this workspace uses: the Linear-style Playbook installed here — \
                  its issue and cycle types, the Plays that automate and gate them, its saved \
                  views, and the schema ids they were actually created under.",
    exclusion: None,
    tools: DEFAULT_TOOLS,
    applies_to: &["issue", "cycle"],
    body: include_str!("skills/linear/linear-style-workspace.md"),
};

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
        skills: SKILLS,
        overview: OVERVIEW,
        views: vec![issues_by_status_view(), current_cycle_issues_view()],
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

/// Which of an ending cycle's tasks roll over: the unfinished ones.
///
/// Read at `task` scope — `cycle.tasks` targets `task` — so an issue's
/// extended statuses arrive as the base values they map to (`in_review` as
/// `in_progress`, `backlog`/`triage` as `open`), and only the two terminal base
/// values need naming.
const ROLLOVER_TASKS: &str = "trigger.node.tasks.where(status != 'done' && status != 'cancelled')";

/// Close out an ending cycle: create its successor, then move its unfinished
/// tasks into it.
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
/// The reassignment is add-then-remove. `cycle.tasks`' reverse cardinality is
/// `one`, enforced on write by replacing the edge from the task's previous
/// cycle, so a task is never in two cycles and a `sum(cycle.tasks, estimate)`
/// never double-counts. The removal states the move in the play itself rather
/// than leaving it to that replacement.
///
/// Add before remove, deliberately: a failure between the two leaves the task
/// in both cycles, which is visible and repairable, rather than in neither,
/// which silently loses it.
///
/// **Only unfinished tasks move.** Done and cancelled tasks stay with the
/// ending cycle as its record of what it accomplished, which is how Linear
/// behaves. Both `for_each`s narrow `trigger.node.tasks` with the same
/// `.where` ([`ROLLOVER_TASKS`]) — the add and the remove must iterate the
/// same set, or a task could be added without being removed. Scoping to the
/// cycle needs nothing extra: the path starts at the ending cycle, so it only
/// ever yields that cycle's own tasks.
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
        play_id: CYCLE_ROLLOVER_ID,
        name: "Close out the ending cycle",
        description: "On the day a cycle ends, create its successor — starting the next day and \
             spanning that cycle's own duration_days — then move the ending cycle's unfinished \
             tasks into it. Done and cancelled tasks stay with the ending cycle as its record.",
        rules: json!([{
            "name": "create-successor-and-roll-over",
            "description":
                "On the day a cycle ends, create the next cycle and move the unfinished \
                 tasks into it",
            "trigger": {
                "type": "scheduled",
                "cron": DAILY_AFTER_MIDNIGHT,
                "select": { "target_type": "cycle" },
            },
            "conditions": [{
                "expr": "node.end_date == today()",
                "description": "The cycle ends today",
            }],
            "actions": [
                {
                    "action_type": "create_node",
                    "description":
                        "Create the next cycle, starting the day after this one ends and \
                         lasting as many days",
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
                    "description": "Add each unfinished task to the next cycle",
                    "for_each": ROLLOVER_TASKS,
                    "params": {
                        "source_id": "{actions[0].result.id}",
                        "relationship_type": "tasks",
                        "target_id": "{item.id}",
                    },
                },
                {
                    "action_type": "remove_relationship",
                    "description": "Take each unfinished task out of the ending cycle",
                    "for_each": ROLLOVER_TASKS,
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
/// transaction and can veto the write (ADR-060). A reactive rule could
/// only complain after the fact.
///
/// Sub-issues are ordinary `has_child` children — no schema models them, and
/// this gate works for any nesting the outline allows.
fn sub_issue_completion_gate() -> PlayStep {
    PlayStep {
        play_id: SUB_ISSUE_GATE_ID,
        name: "Block closing an issue with open sub-issues",
        description:
            "Rejects a status change to done while any child issue is still open. Close the \
             children first, or move them out from under this issue.",
        rules: json!([{
            "name": "reject-done-with-open-children",
            "description": "Refuse to mark an issue done while it has open sub-issues",
            "class": "invariant",
            "trigger": {
                "type": "graph_event",
                "on": "property_changed",
                "select": { "target_type": "issue" },
                // Namespaced, not bare: `update_node` stores a schema field
                // under the node's own type object and reports the change as
                // `"{node_type}.{field}"`, so that is what a trigger's
                // `property_key` matches against. A bare "status" matches
                // nothing and the rule silently never fires.
                "property_key": "issue.status",
            },
            "conditions": [
                {
                    "expr": "node.status == 'done'",
                    "description": "The issue is being marked done",
                },
                {
                    "expr":
                        "node.has_child.exists(c, c.status != 'done' && c.status != 'cancelled')",
                    "description": "At least one sub-issue is neither done nor cancelled",
                },
            ],
            "actions": [{
                "action_type": "reject",
                "description": "Refuse the change and say the sub-issues must be closed first",
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
/// Uses the `blocked_by` reverse edge (the `task` schema declares the pair, and
/// conditions can traverse reverse edges). Fires on the
/// transition into `in_progress` specifically — an issue may sit in `backlog`
/// or `triage` behind a blocker quite legitimately.
fn blocker_gate() -> PlayStep {
    PlayStep {
        play_id: BLOCKER_GATE_ID,
        name: "Block starting an issue with an open blocker",
        description:
            "Rejects a status change to in_progress while anything blocking this issue is \
             still open. Resolve the blocker, or drop the blocks edge if it no longer applies.",
        rules: json!([{
            "name": "reject-start-with-open-blocker",
            "description": "Refuse to start an issue while something blocking it is unfinished",
            "class": "invariant",
            "trigger": {
                "type": "graph_event",
                "on": "property_changed",
                "select": { "target_type": "issue" },
                // Namespaced, not bare: `update_node` stores a schema field
                // under the node's own type object and reports the change as
                // `"{node_type}.{field}"`, so that is what a trigger's
                // `property_key` matches against. A bare "status" matches
                // nothing and the rule silently never fires.
                "property_key": "issue.status",
            },
            "conditions": [
                {
                    "expr": "node.status == 'in_progress'",
                    "description": "The issue is being started",
                },
                {
                    "expr":
                        "node.blocked_by.exists(b, b.status != 'done' && b.status != 'cancelled')",
                    "description": "At least one blocker is neither done nor cancelled",
                },
            ],
            "actions": [{
                "action_type": "reject",
                "description": "Refuse the change and say the blocker must be resolved first",
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
// Views
// ---------------------------------------------------------------------------

/// Issues by Status — the board Linear opens on.
///
/// Grouped by the inherited `status`, so the columns are the extended
/// vocabulary (`backlog`, `triage`, `in_review` alongside the base values)
/// rather than anything this view has to declare. Their order is declared:
/// the enum lists the base values before the extended ones, and the board
/// reads as a workflow, left to right.
fn issues_by_status_view() -> ViewStep {
    ViewStep {
        view_id: ISSUES_BY_STATUS_ID,
        name: "Issues by Status",
        definition: QueryDefinition {
            target_type: "issue".to_string(),
            filters: vec![],
            sorting: None,
            limit: None,
        },
        view_config: json!({
            "lastView": "kanban",
            "kanban": {
                "groupBy": "status",
                "columnOrder": { "status": ISSUE_STATUS_COLUMN_ORDER },
            },
        }),
    }
}

/// The issue boards' columns, in workflow order.
const ISSUE_STATUS_COLUMN_ORDER: [&str; 7] = [
    "triage",
    "backlog",
    "open",
    "in_progress",
    "in_review",
    "done",
    "cancelled",
];

/// Current Cycle Issues — the work in the cycle that spans today, as a board.
///
/// A cycle has no stored status (see [`cycle_schema`]), so "current" is said
/// with dates relative to the day the board is opened (ADR-091): the issue's
/// cycle started on or before today and ends on or after it. An issue is in
/// at most one cycle, so both filters constrain the same one.
fn current_cycle_issues_view() -> ViewStep {
    ViewStep {
        view_id: CURRENT_CYCLE_ISSUES_ID,
        name: "Current Cycle Issues",
        definition: QueryDefinition {
            target_type: "issue".to_string(),
            filters: view_filters(json!([
                {
                    "type": "related", "operator": "exists", "path": ["cycle"],
                    "filter": {
                        "type": "property", "operator": "lte", "property": "start_date",
                        "relative_date": { "anchor": "today" },
                    },
                },
                {
                    "type": "related", "operator": "exists", "path": ["cycle"],
                    "filter": {
                        "type": "property", "operator": "gte", "property": "end_date",
                        "relative_date": { "anchor": "today" },
                    },
                },
            ])),
            sorting: None,
            limit: None,
        },
        view_config: json!({
            "lastView": "kanban",
            "kanban": {
                "groupBy": "status",
                "columnOrder": { "status": ISSUE_STATUS_COLUMN_ORDER },
            },
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::markdown::SeedTier;

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

    /// The viewer ignores an ordered value the enum does not have and appends
    /// one the order leaves out, so a typo or a status added later would
    /// misplace a column without failing anything.
    #[test]
    fn the_issue_board_orders_every_status_and_nothing_else() {
        let ext = issue_status_values();
        let mut statuses: Vec<&str> = vec!["open", "in_progress", "done", "cancelled"];
        statuses.extend(
            ext.params["add_field_values"][0]["values"]
                .as_array()
                .expect("values array")
                .iter()
                .map(|v| v["value"].as_str().expect("value")),
        );
        statuses.sort_unstable();

        let view = issues_by_status_view();
        let mut ordered: Vec<&str> = view.view_config["kanban"]["columnOrder"]["status"]
            .as_array()
            .expect("the board declares its status column order")
            .iter()
            .map(|v| v.as_str().expect("value"))
            .collect();
        ordered.sort_unstable();

        assert_eq!(ordered, statuses);
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
    fn scheduled_rules_declare_a_cron_and_a_selector() {
        for play in playbook().plays {
            for rule in play.rules.as_array().expect("rules array") {
                if rule["trigger"]["type"] == "scheduled" {
                    assert!(rule["trigger"]["cron"].is_string());
                    assert!(rule["trigger"]["select"]["target_type"].is_string());
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
    /// in `tests/it/methodology_linear_execution_test.rs` runs them and counts
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

        // The play states the move in full: the add, then the removal of the
        // edge from the ending cycle.
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
        // fails at runtime with `unknown binding root: 'node'`. The add and
        // the remove must iterate the SAME narrowed set, or a task could be
        // added to the successor without leaving the ending cycle.
        for action in [add, remove] {
            assert_eq!(
                action["for_each"], ROLLOVER_TASKS,
                "for_each must use an action-binding root"
            );
        }
    }

    #[test]
    fn play_ids_are_unique() {
        let pb = playbook();
        let mut ids: Vec<&str> = pb.plays.iter().map(|p| p.play_id).collect();
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

        for s in skills.iter().map(PlaybookSkill::template) {
            assert_eq!(s.root_node_type, "skill");
            let skill = crate::models::SkillFields::from_properties(&s.root_properties)
                .expect("playbook skill must decode");
            assert!(
                !skill.description.is_empty(),
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

    /// SKILL.md tells an agent to recognise an installed Playbook by a skill
    /// titled `<name> Workspace`. A retitled overview silently breaks that
    /// routing, sending agents in installed workspaces back to the install doc.
    #[test]
    fn overview_is_titled_as_skill_md_routes_on() {
        let pb = playbook();
        let overview = pb
            .overview
            .overview_template(&crate::methodology::skills::InstalledIds::default());
        assert_eq!(overview.title, format!("{} Workspace", pb.name));
        assert!(matches!(overview.tier, SeedTier::Starter));
    }

    /// Playbook content is opt-in, so it must not be seeded at startup with the
    /// System-tier content.
    #[test]
    fn playbook_skills_are_starter_tier() {
        for s in playbook().skills {
            assert!(
                matches!(s.template().tier, SeedTier::Starter),
                "{}",
                s.title
            );
        }
    }
}
