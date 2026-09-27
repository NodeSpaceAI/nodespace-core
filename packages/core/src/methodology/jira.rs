//! Jira-style work tracking: Epics, Stories and Bugs, and stateful Sprints.
//!
//! Models what is structurally distinct about Jira's workflow — an issue-type
//! hierarchy with type-specific fields, and a Sprint that is not just a dated
//! container but has its own `future` → `active` → `closed` lifecycle, with
//! field locking once it closes. As with every playbook, nothing here is
//! platform code; it is a composition of `extends`, declared relationships and
//! Plays.
//!
//! # Issue types are real types
//!
//! `story`, `bug` and `epic` each declare `extends: task` (ADR-078), so an
//! instance carries its own `node_type` and fields while inheriting every
//! `task` field and relationship. Jira's plain "Task" issue type is core `task`
//! itself — no wrapper schema — and all four run the same status workflow.
//! Subtype-aware querying means anything written against `task`, including
//! the core parent-completion Play, reaches stories and bugs unchanged.
//!
//! # One relationship per container, targeting `task`
//!
//! An epic groups work through `epic.issues`, a sprint through
//! `sprint.issues`. Each targets `task` rather than being declared three times
//! (once per issue type): a relationship's target type is satisfied by any
//! subtype, so one declaration already accepts a task, story or bug, and three
//! parallel ones would let the same story sit in an epic twice under different
//! names. Epic membership is Jira's flat "Epic Link", not outline nesting —
//! `has_child` stays reserved for sub-tasks.
//!
//! # A sprint's status is stored, unlike a cycle's
//!
//! The Linear playbook's `cycle` derives upcoming/active/past from its dates.
//! A Jira sprint cannot: it starts and closes when someone says so, not when a
//! date passes, and closing it is an event with consequences (the close-lock).
//! So `sprint_status` is a stored field, and the Plays below keep its
//! transitions honest. It is `sprint_status` rather than `status` so it never
//! reads as the task lifecycle to a consumer that knows that name.

use crate::methodology::skills::playbook_skill;
use crate::methodology::{MethodologyPlaybook, PlayStep, SchemaStep, ViewStep};
use crate::services::QueryDefinition;
use serde_json::json;

/// The Jira-style playbook.
pub fn playbook() -> MethodologyPlaybook {
    MethodologyPlaybook {
        id: "jira",
        name: "Jira-style",
        description:
            "Epics, Stories and Bugs on top of tasks, with story points and bug severity, and \
             Sprints that move future → active → closed. A sprint cannot start without dates or \
             be reopened once closed, and a closed sprint's dates and issues are locked.",
        schemas: vec![story_schema(), bug_schema(), epic_schema(), sprint_schema()],
        field_value_extensions: vec![],
        plays: vec![
            sprint_transition_gate(),
            sprint_completion_stamp(),
            sprint_close_lock(),
        ],
        skills: vec![
            playbook_skill(include_str!(
                "skills/jira/creating-epics-stories-and-bugs.md"
            )),
            playbook_skill(include_str!("skills/jira/working-with-sprints.md")),
            playbook_skill(include_str!("skills/jira/sprint-validation-rules.md")),
        ],
        overview: include_str!("skills/jira/jira-style-workspace.md"),
        views: vec![epics_by_status_view(), sprints_by_status_view()],
    }
}

// ---------------------------------------------------------------------------
// Schemas
// ---------------------------------------------------------------------------

/// `story` — a user-facing requirement, sized in story points.
fn story_schema() -> SchemaStep {
    SchemaStep {
        schema_id: "story",
        params: json!({
            "name": "Story",
            "extends": "task",
            "description":
                "A requirement described from the user's point of view, in a Jira-style \
                 workflow. Extends task with story points. Grouped into an epic via \
                 epic.issues and planned into sprints via sprint.issues.",
            "fields": [{
                "name": "story_points",
                "friendlyName": "Story points",
                "type": "number",
                "protection": "user",
                "indexed": true,
                "required": false,
                "description":
                    "Relative size of the story. A number rather than a fixed scale: teams \
                     choose their own — Fibonacci, powers of two, or plain integers — and a \
                     closed list would fight whichever one they use.",
            }],
        }),
    }
}

/// `bug` — a defect, with how bad it is kept apart from how urgent it is.
fn bug_schema() -> SchemaStep {
    SchemaStep {
        schema_id: "bug",
        params: json!({
            "name": "Bug",
            "extends": "task",
            "description":
                "A defect, in a Jira-style workflow. Extends task with severity and the \
                 environment it was seen in. Severity is how bad the defect is; the inherited \
                 priority is how urgently to fix it — separate axes, separate fields.",
            "fields": [
                {
                    "name": "severity",
                    "friendlyName": "Severity",
                    "type": "enum",
                    "protection": "user",
                    "indexed": true,
                    "required": false,
                    "extensible": true,
                    "coreValues": [
                        { "value": "critical", "label": "Critical" },
                        { "value": "major", "label": "Major" },
                        { "value": "minor", "label": "Minor" },
                        { "value": "trivial", "label": "Trivial" },
                    ],
                    "userValues": [],
                    "description":
                        "How bad the defect is: critical (data loss, outage, no workaround), \
                         major (a feature broken), minor (broken with a workaround), trivial \
                         (cosmetic). Independent of priority — a trivial bug on the landing \
                         page can be urgent.",
                },
                {
                    "name": "environment",
                    "friendlyName": "Environment",
                    "type": "string",
                    "protection": "user",
                    "indexed": false,
                    "required": false,
                    "description":
                        "Where the defect was seen — deployment, browser and OS, version. \
                         Free text: environments are open-ended.",
                },
            ],
        }),
    }
}

/// `epic` — a large body of work grouping tasks, stories and bugs.
fn epic_schema() -> SchemaStep {
    SchemaStep {
        schema_id: "epic",
        params: json!({
            "name": "Epic",
            "extends": "task",
            "description":
                "A large body of work, in a Jira-style workflow, grouping the tasks, stories \
                 and bugs that deliver it through epic.issues. Extends task, so it carries its \
                 own status, assignee and priority.",
            "fields": [{
                "name": "target_date",
                "friendlyName": "Target date",
                "type": "date",
                "protection": "user",
                "indexed": true,
                "required": false,
                "description":
                    "When the epic is expected to be delivered. A target, not a deadline \
                     enforced by anything.",
            }],
            "relationships": [{
                "name": "issues",
                "targetType": "task",
                "direction": "out",
                "cardinality": "many",
                "reverseName": "epic",
                "reverseCardinality": "one",
                "description":
                    "The work in this epic — Jira's Epic Link. Targets task, so tasks, stories \
                     and bugs can all belong. An issue is in at most one epic; linking it to \
                     another moves it. Flat membership, not nesting: sub-tasks use the outline.",
            }],
        }),
    }
}

/// `sprint` — a time-boxed iteration with an explicit lifecycle.
///
/// `start_date` and `end_date` are optional at creation — a future sprint is
/// often created before its dates are known — and required only to start it,
/// which [`sprint_transition_gate`] enforces.
fn sprint_schema() -> SchemaStep {
    SchemaStep {
        schema_id: "sprint",
        params: json!({
            "name": "Sprint",
            "description":
                "A time-boxed iteration with its own lifecycle: future while being planned, \
                 active once started, closed once completed. A closed sprint is a record — only \
                 its name and goal can still change.",
            "fields": [
                {
                    "name": "sprint_status",
                    "friendlyName": "Sprint status",
                    "type": "enum",
                    "protection": "user",
                    "indexed": true,
                    "required": false,
                    "extensible": false,
                    "default": "future",
                    "coreValues": [
                        { "value": "future", "label": "Future" },
                        { "value": "active", "label": "Active" },
                        { "value": "closed", "label": "Closed" },
                    ],
                    "userValues": [],
                    "description":
                        "future while being planned; active once started, which needs both \
                         dates; closed once completed, which is final. Moves only \
                         future → active → closed.",
                },
                {
                    "name": "start_date",
                    "friendlyName": "Start date",
                    "type": "date",
                    "protection": "user",
                    "indexed": true,
                    "required": false,
                    "description":
                        "When the sprint is planned to start. May be set ahead of time and \
                         moved until the sprint closes.",
                },
                {
                    "name": "end_date",
                    "friendlyName": "End date",
                    "type": "date",
                    "protection": "user",
                    "indexed": true,
                    "required": false,
                    "description":
                        "When the sprint is planned to end — the anticipated close, not the \
                         actual one, which is completed_date.",
                },
                {
                    "name": "completed_date",
                    "friendlyName": "Completed",
                    "type": "date",
                    "protection": "user",
                    "indexed": false,
                    "required": false,
                    "description":
                        "When the sprint was actually closed. Set automatically on close; \
                         never set it by hand.",
                },
                {
                    "name": "goal",
                    "friendlyName": "Goal",
                    "type": "string",
                    "protection": "user",
                    "indexed": false,
                    "required": false,
                    "description":
                        "What the sprint sets out to achieve, in a sentence. Stays editable \
                         after the sprint closes.",
                },
            ],
            "relationships": [{
                "name": "issues",
                "targetType": "task",
                "direction": "out",
                "cardinality": "many",
                "reverseName": "sprint",
                "reverseCardinality": "many",
                "description":
                    "The work planned into this sprint. Targets task, so tasks, stories and \
                     bugs can all be planned. Many on both ends: unfinished work is carried \
                     into later sprints while staying on the closed ones' record.",
            }],
        }),
    }
}

// ---------------------------------------------------------------------------
// Plays
// ---------------------------------------------------------------------------

/// A condition true when this update changed `sprint_status` from one of
/// `from` to `to`.
///
/// Reads `trigger.properties`, not `trigger.property`: the latter is the
/// update's FIRST changed property, which is some other field whenever an
/// update sets several at once — starting a sprint and its dates together
/// is the ordinary case. A missing old value is a sprint created without the
/// schema default, and counts as `future`.
fn status_moved(from: &[&str], to: &str) -> String {
    let from = from
        .iter()
        .map(|v| match *v {
            "null" => "p.old_value == null".to_string(),
            v => format!("p.old_value == '{v}'"),
        })
        .collect::<Vec<_>>()
        .join(" || ");
    format!(
        "trigger.properties.exists(p, p.key == 'sprint.sprint_status' \
         && ({from}) && p.new_value == '{to}')"
    )
}

/// Keep a sprint's lifecycle to future → active → closed, and a sprint's
/// start to having dates.
///
/// Mirrors the transitions Jira's own sprint API allows. The transition rule
/// is an allow-list: any `sprint_status` change that is not one of the three
/// legal moves is rejected, which covers reopening a closed sprint, restarting
/// an active one as future, and clearing the field.
///
/// Creation is gated too, because the transition rule only sees updates: a
/// sprint created already active would skip the date check, and one created
/// closed would skip completion.
fn sprint_transition_gate() -> PlayStep {
    let legal = [
        status_moved(&["null"], "future"),
        status_moved(&["future", "null"], "active"),
        status_moved(&["active"], "closed"),
    ]
    .join(" || ");

    PlayStep {
        play_id: "jira-sprint-transition-gate",
        name: "Keep sprints moving future → active → closed",
        description:
            "Rejects any sprint_status change other than future → active or active → closed, \
             rejects starting a sprint without both dates, and rejects creating a sprint that \
             is already started or closed.",
        rules: json!([
            {
                "name": "reject-illegal-sprint-transition",
                "class": "invariant",
                "trigger": {
                    "type": "graph_event",
                    "on": "property_changed",
                    "node_type": "sprint",
                    // Namespaced: a trigger's property_key matches the
                    // `{node_type}.{field}` form `update_node` reports.
                    "property_key": "sprint.sprint_status",
                },
                "conditions": [format!("!({legal})")],
                "actions": [{
                    "action_type": "reject",
                    "params": {
                        "message":
                            "A sprint moves only from future to active, and from active to \
                             closed. A closed sprint cannot be reopened — plan the remaining \
                             work into a new sprint instead.",
                    },
                }],
            },
            {
                "name": "reject-starting-without-dates",
                "class": "invariant",
                "trigger": {
                    "type": "graph_event",
                    "on": "property_changed",
                    "node_type": "sprint",
                    "property_key": "sprint.sprint_status",
                },
                "conditions": [
                    "node.sprint_status == 'active'",
                    "!has(node.start_date) || !has(node.end_date)",
                ],
                "actions": [{
                    "action_type": "reject",
                    "params": {
                        "message":
                            "A sprint cannot start without a start_date and an end_date. Set \
                             both, then start it — they can be in the same update.",
                    },
                }],
            },
            {
                "name": "reject-creating-a-started-sprint",
                "class": "invariant",
                "trigger": {
                    "type": "graph_event",
                    "on": "node_created",
                    "node_type": "sprint",
                },
                "conditions": [
                    "(has(node.sprint_status) && node.sprint_status != 'future') \
                     || has(node.completed_date)",
                ],
                "actions": [{
                    "action_type": "reject",
                    "params": {
                        "message":
                            "A sprint is created as future, with no completed_date. Create \
                             it, then start it once its dates are set.",
                    },
                }],
            },
        ]),
    }
}

/// Stamp `completed_date` when a sprint closes.
///
/// Reactive, not invariant: a stamp is a consequence of the close, not a
/// condition on it, and an invariant rule may not update its own trigger node
/// (ADR-060 §2 — it would re-fire itself). Nothing is lost by running after
/// commit, since [`sprint_transition_gate`] has already accepted the close.
///
/// The value is the sprint's own `modifiedAt` — the moment of the write that
/// closed it — rather than a wall-clock read at execution: action bindings
/// have no clock, and a value already on the trigger node is the same on
/// every device that runs this.
fn sprint_completion_stamp() -> PlayStep {
    PlayStep {
        play_id: "jira-sprint-completion-stamp",
        name: "Record when a sprint closes",
        description: "When a sprint moves from active to closed, sets its completed_date to the \
             time of the close.",
        rules: json!([{
            "name": "stamp-completed-date",
            "trigger": {
                "type": "graph_event",
                "on": "property_changed",
                "node_type": "sprint",
                "property_key": "sprint.sprint_status",
            },
            "conditions": [status_moved(&["active"], "closed")],
            "actions": [{
                "action_type": "update_node",
                "params": {
                    "node_id": "{trigger.node.id}",
                    "properties": { "completed_date": "{trigger.node.modifiedAt}" },
                },
            }],
        }]),
    }
}

/// Freeze a closed sprint, except for its name and goal.
///
/// Jira ignores edits to a closed sprint's dates; here they are rejected
/// instead, so the caller learns the write did not land. The status field is
/// already covered — [`sprint_transition_gate`] rejects every move out of
/// `closed`.
///
/// `completed_date` accepts exactly one write: the [`sprint_completion_stamp`]
/// on a closed sprint whose date is still unset. Setting it on an open sprint,
/// or changing it once stamped, is rejected.
///
/// Membership is locked through `relationship_added`/`relationship_removed`
/// triggers on the sprint — the edge's source, whichever node the caller
/// started from.
fn sprint_close_lock() -> PlayStep {
    let mut rules: Vec<serde_json::Value> = ["start_date", "end_date"]
        .iter()
        .map(|field| {
            json!({
                "name": format!("lock-closed-sprint-{field}"),
                "class": "invariant",
                "trigger": {
                    "type": "graph_event",
                    "on": "property_changed",
                    "node_type": "sprint",
                    "property_key": format!("sprint.{field}"),
                },
                "conditions": ["node.sprint_status == 'closed'"],
                "actions": [{
                    "action_type": "reject",
                    "params": {
                        // Names the field, which also keeps each rule's
                        // action list distinct: byte-identical actions in one
                        // play derive the same rule identity and are refused.
                        "message": format!(
                            "This sprint is closed, so its {field} is locked as the record of \
                             what was planned. Only its name and goal can still change."
                        ),
                    },
                }],
            })
        })
        .collect();

    rules.push(json!({
        "name": "lock-completed-date",
        "class": "invariant",
        "trigger": {
            "type": "graph_event",
            "on": "property_changed",
            "node_type": "sprint",
            "property_key": "sprint.completed_date",
        },
        "conditions": [
            "node.sprint_status != 'closed' || trigger.properties.exists(p, \
             p.key == 'sprint.completed_date' && p.old_value != null)",
        ],
        "actions": [{
            "action_type": "reject",
            "params": {
                "message":
                    "completed_date records when the sprint was closed and is set \
                     automatically at that moment. It cannot be set by hand or changed \
                     afterwards.",
            },
        }],
    }));

    for (on, verb) in [
        ("relationship_added", "added to"),
        ("relationship_removed", "removed from"),
    ] {
        rules.push(json!({
            "name": format!("lock-closed-sprint-membership-{on}"),
            "class": "invariant",
            "trigger": {
                "type": "graph_event",
                "on": on,
                "node_type": "sprint",
            },
            "conditions": [
                "trigger.relationship.name == 'issues'",
                "node.sprint_status == 'closed'",
            ],
            "actions": [{
                "action_type": "reject",
                "params": {
                    "message": format!(
                        "This sprint is closed, so work cannot be {verb} it — its issues \
                         are the record of what the sprint held. Plan unfinished work into \
                         a future sprint instead."
                    ),
                },
            }],
        }));
    }

    PlayStep {
        play_id: "jira-sprint-close-lock",
        name: "Lock closed sprints",
        description: "Rejects changing a closed sprint's dates or its issues, and rejects setting \
             completed_date by hand. A closed sprint's name and goal stay editable.",
        rules: serde_json::Value::Array(rules),
    }
}

// ---------------------------------------------------------------------------
// Views
// ---------------------------------------------------------------------------

/// Epics by Status — the roadmap at a glance.
fn epics_by_status_view() -> ViewStep {
    ViewStep {
        view_id: "jira-epics-by-status",
        name: "Epics by Status",
        definition: QueryDefinition {
            target_type: "epic".to_string(),
            filters: vec![],
            sorting: None,
            limit: None,
        },
        view_config: json!({
            "lastView": "kanban",
            "kanban": { "groupBy": "status" },
        }),
    }
}

/// Sprints by lifecycle: what is being planned, what is running, what is done.
fn sprints_by_status_view() -> ViewStep {
    ViewStep {
        view_id: "jira-sprints-by-status",
        name: "Sprints",
        definition: QueryDefinition {
            target_type: "sprint".to_string(),
            filters: vec![],
            sorting: None,
            limit: None,
        },
        view_config: json!({
            "lastView": "kanban",
            "kanban": { "groupBy": "sprint_status" },
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

    #[test]
    fn issue_types_extend_task_with_their_own_fields() {
        for (step, fields) in [
            (story_schema(), vec!["story_points"]),
            (bug_schema(), vec!["severity", "environment"]),
            (epic_schema(), vec!["target_date"]),
        ] {
            assert_eq!(step.params["extends"], "task", "{}", step.schema_id);
            assert_eq!(field_names(&step), fields, "{}", step.schema_id);
        }
    }

    /// Story points are a number, not an enum: teams choose their own scale.
    #[test]
    fn story_points_is_a_number() {
        assert_eq!(story_schema().params["fields"][0]["type"], "number");
    }

    #[test]
    fn sprint_is_standalone_with_a_stored_lifecycle() {
        let sprint = sprint_schema();
        assert!(sprint.params.get("extends").is_none());
        let status = &sprint.params["fields"][0];
        assert_eq!(status["name"], "sprint_status");
        assert_eq!(status["default"], "future");
        assert_eq!(status["extensible"], false, "the gates name every value");
        for field in ["start_date", "end_date", "completed_date", "goal"] {
            assert!(field_names(&sprint).contains(&field.to_string()), "{field}");
        }
    }

    /// One declaration per container, targeting `task`: any subtype satisfies
    /// it, so declaring it per issue type would only duplicate membership.
    #[test]
    fn containers_declare_one_relationship_targeting_task() {
        for (step, reverse, reverse_cardinality) in [
            (epic_schema(), "epic", "one"),
            (sprint_schema(), "sprint", "many"),
        ] {
            let rels = step.params["relationships"].as_array().expect("rels");
            assert_eq!(rels.len(), 1, "{}", step.schema_id);
            let r = &rels[0];
            assert_eq!(r["name"], "issues");
            assert_eq!(r["targetType"], "task");
            assert_eq!(r["cardinality"], "many");
            assert_eq!(r["reverseName"], reverse);
            assert_eq!(r["reverseCardinality"], reverse_cardinality);
        }
    }

    #[test]
    fn rules_using_reject_are_declared_invariant() {
        for play in playbook().plays {
            for rule in play.rules.as_array().expect("rules array") {
                let uses_reject = rule["actions"]
                    .as_array()
                    .is_some_and(|a| a.iter().any(|x| x["action_type"] == "reject"));
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
    fn play_ids_are_unique() {
        let pb = playbook();
        let mut ids: Vec<&str> = pb.plays.iter().map(|p| p.play_id).collect();
        ids.sort_unstable();
        let before = ids.len();
        ids.dedup();
        assert_eq!(before, ids.len(), "play ids must be unique");
    }

    #[test]
    fn skills_are_narrow_starter_tier_and_carry_guidance() {
        let skills = playbook().skills;
        assert!(skills.len() >= 3, "expected several narrow skills");
        for s in &skills {
            assert_eq!(s.root_node_type, "skill");
            assert!(matches!(s.tier, SeedTier::Starter), "{}", s.title);
            let skill = crate::models::SkillNode::from_properties(&s.title, &s.root_properties)
                .expect("playbook skill must decode");
            assert!(!skill.description.is_empty(), "{}", s.title);
            assert!(s.markdown_content.contains('#'), "{}", s.title);
            assert!(s.child_node_type.is_none());
        }
    }

    /// SKILL.md routes an agent to an installed Playbook by a skill titled
    /// `<name> Workspace`.
    #[test]
    fn overview_is_titled_as_skill_md_routes_on() {
        let pb = playbook();
        let overview = crate::methodology::skills::playbook_overview_skill(
            pb.overview,
            &crate::methodology::skills::InstalledIds::default(),
        );
        assert_eq!(overview.title, format!("{} Workspace", pb.name));
    }
}
