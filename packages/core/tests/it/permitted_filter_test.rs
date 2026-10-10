//! The `permitted` query filter (ADR-094 §9), against the gate Plays every
//! database is seeded with and a running engine.
//!
//! A "what can be started" view states one thing: open tasks for which the
//! change to `in_progress` would not be rejected. What a task needs before it
//! may start is the Plays' to say, so the view follows them: nothing here
//! restates a blocker or a plan condition in a filter.

use crate::core_model_plays_test::Harness;
use anyhow::Result;
use nodespace_core::models::Node;
use nodespace_core::ops::node_context_ops::{read_node_context, NodeContextInput};
use nodespace_core::ops::query_ops::{
    count_query, run_saved_query_nodes, RunSavedQueryInput, SavedQueryRun,
};
use nodespace_core::playbook::core_plays::{
    task_blockers_rules, TASK_BLOCKERS_PLAY_ID, TASK_LINEAGE_PLAY_ID,
};
use nodespace_core::services::DryRunVerdict;
use serde_json::{json, Value};
use std::time::Duration;

const READY: &str = "e1000000-0000-4000-8000-000000000001";

fn open_tasks() -> Value {
    json!({ "type": "property", "operator": "equals", "property": "status", "value": "open" })
}

fn may_start() -> Value {
    json!({ "type": "permitted", "operator": "equals", "property": "status", "value": "in_progress" })
}

/// "Startable tasks" as a view over the rules: open, and startable.
async fn save_ready_query(h: &Harness, extra: Value) -> Result<()> {
    let mut fields = json!({ "target_type": "task", "filters": [open_tasks(), may_start()] });
    for (key, value) in extra.as_object().into_iter().flatten() {
        fields[key] = value.clone();
    }
    h.service
        .create_node(Node::new_with_id(
            READY.to_string(),
            "query".to_string(),
            "Startable tasks".to_string(),
            fields,
        ))
        .await?;
    Ok(())
}

async fn run(h: &Harness, input: Value) -> Result<SavedQueryRun> {
    let input: RunSavedQueryInput = serde_json::from_value(input)?;
    Ok(run_saved_query_nodes(&h.service, input).await?)
}

async fn ready(h: &Harness) -> Result<Vec<String>> {
    let run = run(h, json!({ "query": READY })).await?;
    assert_eq!(run.unresolved, 0, "every candidate's rules resolve");
    Ok(sorted(run.nodes.into_iter().map(|n| n.id)))
}

fn sorted(ids: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut ids: Vec<String> = ids.into_iter().collect();
    ids.sort();
    ids
}

/// The view keeps exactly the open tasks the gate Plays would let start, and
/// follows the graph as the rules read it: finishing a blocker brings the
/// task it blocked into the view with no change to the query.
#[tokio::test]
async fn a_ready_view_keeps_the_tasks_the_gate_plays_would_let_start() -> Result<()> {
    let h = Harness::start().await?;
    let free = h.task().await?;
    let blocker = h.task().await?;
    let blocked = h.task().await?;
    h.link(&blocker, "blocks", &blocked).await?;
    // A task whose plan is approved and whose spec is linked may start.
    let traced = h.traced_task().await?.task;
    // A task carrying out a plan that is not approved may not.
    let draft_plan = h.create("plan", json!({})).await?;
    let drafted = h.task().await?;
    h.link(&draft_plan, "tasks", &drafted).await?;
    save_ready_query(&h, json!({})).await?;

    assert_eq!(
        ready(&h).await?,
        sorted([free.clone(), blocker.clone(), traced.clone()])
    );

    // The view agrees with the write it predicts, task by task.
    for (task, starts) in [(&free, true), (&blocked, false), (&drafted, false)] {
        let node = h.service.get_node(task).await?.expect("task exists");
        let verdict = h
            .service
            .dry_run_update(
                &node,
                nodespace_core::models::NodeUpdate::default()
                    .with_properties(json!({ "status": "in_progress" })),
            )
            .await?;
        assert_eq!(verdict.is_allowed(), starts, "{verdict:?}");
        if !starts {
            assert!(matches!(verdict, DryRunVerdict::Rejected { .. }));
        }
    }

    h.set_status(&blocker, "done").await?;
    assert_eq!(
        ready(&h).await?,
        sorted([free.clone(), blocked.clone(), traced.clone()]),
        "a finished blocker is not open, and no longer blocks"
    );

    // What the view lists can be started.
    h.set_status(&blocked, "in_progress").await?;
    h.stop().await;
    Ok(())
}

/// Switching a gate Play off, or editing its rule, changes what the view
/// keeps. The query is not touched.
#[tokio::test]
async fn switching_off_or_editing_a_gate_play_changes_what_the_view_keeps() -> Result<()> {
    let h = Harness::start().await?;
    let blocker = h.task().await?;
    let blocked = h.task().await?;
    h.link(&blocker, "blocks", &blocked).await?;
    let draft_plan = h.create("plan", json!({})).await?;
    let drafted = h.task().await?;
    h.link(&draft_plan, "tasks", &drafted).await?;
    save_ready_query(&h, json!({})).await?;
    let stored = h.service.get_node(READY).await?.expect("query exists");

    assert_eq!(ready(&h).await?, sorted([blocker.clone()]));

    // Off: the lineage rule no longer refuses a task with a draft plan.
    h.set(TASK_LINEAGE_PLAY_ID, json!({ "enabled": false }))
        .await?;
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert_eq!(ready(&h).await?, sorted([blocker.clone(), drafted.clone()]));

    // Edited: the blockers rule now guards only the move to review, so a
    // blocked task may be started.
    let mut rules = task_blockers_rules();
    rules[0]["conditions"][0] = json!({
        "expr": "node.status == 'in_review'",
        "description": "The task is being put in review",
    });
    h.set(TASK_BLOCKERS_PLAY_ID, json!({ "rules": rules }))
        .await?;
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert_eq!(
        ready(&h).await?,
        sorted([blocker.clone(), blocked.clone(), drafted.clone()])
    );

    assert_eq!(
        h.service.get_node(READY).await?.expect("query exists"),
        stored,
        "the saved query itself is unchanged"
    );
    h.stop().await;
    Ok(())
}

/// The filter is asked after the query's other filters, and sort and limit
/// apply to what it keeps: a task it leaves out does not take up a place in
/// a limited result.
#[tokio::test]
async fn sort_and_limit_apply_to_what_the_filter_keeps() -> Result<()> {
    let h = Harness::start().await?;
    let mut by_priority = Vec::new();
    for priority in ["highest", "high", "medium", "low"] {
        by_priority.push(
            h.create(
                "task",
                json!({ "status": "open", "priority": priority, "requires_spec": false }),
            )
            .await?,
        );
    }
    // The most urgent task is blocked, by a task the other filters leave out.
    let blocker = h.create("task", json!({ "status": "in_progress" })).await?;
    h.link(&blocker, "blocks", &by_priority[0]).await?;
    save_ready_query(
        &h,
        json!({ "sorting": [{ "field": "priority", "direction": "asc" }], "limit": 2 }),
    )
    .await?;

    let run = run(&h, json!({ "query": READY })).await?;
    let ids: Vec<String> = run.nodes.into_iter().map(|n| n.id).collect();
    assert_eq!(ids, by_priority[1..3], "the next two by priority, in order");

    // A run-time limit lowers it the same way.
    let one = run_ids_in_order(&h, json!({ "query": READY, "limit": 1 })).await?;
    assert_eq!(one, by_priority[1..2]);
    h.stop().await;
    Ok(())
}

async fn run_ids_in_order(h: &Harness, input: Value) -> Result<Vec<String>> {
    Ok(run(h, input)
        .await?
        .nodes
        .into_iter()
        .map(|n| n.id)
        .collect())
}

/// Negated, the filter keeps the nodes the change would be rejected for: a
/// "blocked" view. It can also be given at run time, ANDed with a saved
/// query's own filters, and a count agrees with the run.
#[tokio::test]
async fn the_filter_can_be_negated_given_at_run_time_and_counted() -> Result<()> {
    let h = Harness::start().await?;
    let free = h.task().await?;
    let blocker = h.task().await?;
    let blocked = h.task().await?;
    h.link(&blocker, "blocks", &blocked).await?;
    h.service
        .create_node(Node::new_with_id(
            READY.to_string(),
            "query".to_string(),
            "Open tasks".to_string(),
            json!({ "target_type": "task", "filters": [open_tasks()] }),
        ))
        .await?;

    let mut cannot_start = may_start();
    cannot_start["negate"] = json!(true);
    let stuck = run(
        &h,
        json!({ "query": READY, "filters": [cannot_start.clone()] }),
    )
    .await?;
    assert_eq!(
        sorted(stuck.nodes.into_iter().map(|n| n.id)),
        std::slice::from_ref(&blocked)
    );
    let startable = run(
        &h,
        json!({ "query": "Open tasks", "filters": [may_start()] }),
    )
    .await?;
    assert_eq!(
        sorted(startable.nodes.into_iter().map(|n| n.id)),
        sorted([free, blocker])
    );

    let count = |filter: Value| {
        let service = h.service.clone();
        async move {
            count_query(
                &service,
                serde_json::from_value(json!({
                    "target_type": "task", "filters": [open_tasks(), filter]
                }))
                .unwrap(),
            )
            .await
            .unwrap()
        }
    };
    assert_eq!(count(may_start()).await, 2);
    assert_eq!(count(cannot_start).await, 1);
    h.stop().await;
    Ok(())
}

/// A node is a member of a view with the filter only when the filter keeps
/// it, so a skill attached to the view applies to the tasks that can start
/// and not to the blocked one.
#[tokio::test]
async fn membership_of_a_view_follows_the_filter() -> Result<()> {
    let h = Harness::start().await?;
    let free = h.task().await?;
    let blocker = h.task().await?;
    let blocked = h.task().await?;
    h.link(&blocker, "blocks", &blocked).await?;
    save_ready_query(&h, json!({})).await?;
    let skill = h
        .service
        .create_node(
            nodespace_core::models::SkillFields::new(
                "How to carry out a ready task",
                &["get_node"],
                2,
            )
            .into_node("Implementing a task"),
        )
        .await?;
    h.link(&skill, "attached_to", READY).await?;

    let carries_the_skill = |task: String| {
        let service = h.service.clone();
        let skill = skill.clone();
        async move {
            let context = read_node_context(
                &service,
                NodeContextInput {
                    node_id: task,
                    paths: Vec::new(),
                },
            )
            .await
            .unwrap();
            context
                .attached
                .skills
                .iter()
                .any(|attached| attached.skill.id == skill)
        }
    };
    assert!(carries_the_skill(free).await);
    assert!(!carries_the_skill(blocked).await);
    h.stop().await;
    Ok(())
}

/// A candidate whose rule cannot be evaluated is left out and counted, and
/// the query still answers for the rest.
#[tokio::test]
async fn a_candidate_whose_rules_cannot_be_resolved_is_excluded_and_counted() -> Result<()> {
    let h = Harness::start().await?;
    for definition in [
        json!({ "name": "pf-owner", "fields": [{ "name": "name", "type": "text" }] }),
        json!({
            "name": "pf-item",
            "fields": [
                { "name": "stage", "type": "text" },
                { "name": "kind", "type": "text" }
            ],
            "relationships": [{
                "name": "owner", "targetType": "pf-owner", "direction": "out",
                "cardinality": "one", "reverseName": "items", "reverseCardinality": "many"
            }]
        }),
    ] {
        nodespace_core::schema::handle_create_schema(&h.service, definition).await?;
    }
    // The rejection's message binds the owner's name, which an item with no
    // owner does not have: the rule cannot be evaluated for an odd item.
    h.try_create(
        "play",
        "Odd items need an owner",
        json!({ "rules": [{
            "name": "reject-moving-an-odd-item",
            "class": "invariant",
            "description": "Refuse to move an odd item",
            "trigger": {
                "type": "graph_event", "on": "property_changed",
                "select": { "target_type": "pf-item" }, "property_key": "pf-item.stage"
            },
            "conditions": [{ "expr": "node.kind == 'odd'", "description": "The item is odd" }],
            "actions": [{
                "description": "Refuse, naming the owner",
                "action_type": "reject",
                "params": { "message": "{trigger.node.owner.name}" }
            }]
        }] }),
    )
    .await?;
    tokio::time::sleep(Duration::from_millis(150)).await;

    let mut even = Vec::new();
    for kind in ["even", "odd", "even", "odd", "odd"] {
        let id = h
            .try_create(
                "pf-item",
                "An item",
                json!({ "stage": "wait", "kind": kind }),
            )
            .await?;
        if kind == "even" {
            even.push(id);
        }
    }
    h.service
        .create_node(Node::new_with_id(
            READY.to_string(),
            "query".to_string(),
            "Movable items".to_string(),
            json!({ "target_type": "pf-item", "filters": [{
                "type": "permitted", "operator": "equals", "property": "stage", "value": "go"
            }] }),
        ))
        .await?;

    let run = run(&h, json!({ "query": READY })).await?;
    assert_eq!(run.unresolved, 3, "the three odd items");
    assert_eq!(sorted(run.nodes.into_iter().map(|n| n.id)), sorted(even));
    h.stop().await;
    Ok(())
}

/// The seeded "Ready tasks" queue follows the spec Play: a task with a
/// checklist and nothing else is not ready until it links an approved spec or
/// is marked as not needing one (ADR-097 §6).
#[tokio::test]
async fn the_seeded_ready_queue_leaves_out_a_task_the_spec_play_would_refuse() -> Result<()> {
    use nodespace_core::services::query_service::core_queries::READY_TASKS_QUERY_ID;

    let h = Harness::start().await?;
    let gated = h.spec_gated_task().await?;
    h.child(&gated, "checkbox", "- [ ] It works").await?;
    let small = h.task().await?;
    h.child(&small, "checkbox", "- [ ] It works").await?;
    let specced = h.spec_gated_task().await?;
    h.child(&specced, "checkbox", "- [ ] It works").await?;
    let spec = h.approved_spec().await?;
    h.link(&spec, "tasks", &specced).await?;
    tokio::time::sleep(Duration::from_millis(150)).await;

    let ids = run_ids_in_order(&h, json!({ "query": READY_TASKS_QUERY_ID })).await?;
    assert_eq!(sorted(ids), sorted([small.clone(), specced.clone()]));

    h.set(&gated, json!({ "requires_spec": false })).await?;
    let ids = run_ids_in_order(&h, json!({ "query": READY_TASKS_QUERY_ID })).await?;
    assert_eq!(sorted(ids), sorted([gated, small, specced]));
    h.stop().await;
    Ok(())
}
