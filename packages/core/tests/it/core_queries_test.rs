//! The saved queries every database is seeded with (ADR-092 §7 and §8): the
//! three status lists and the three queues, what "Ready tasks" keeps and in
//! what order, and that a user's edit to one survives a reopen.

use anyhow::Result;
use nodespace_core::db::SqliteStore;
use nodespace_core::markdown::prepare_nodes_from_template;
use nodespace_core::models::{Node, NodeUpdate, QueryFields};
use nodespace_core::ops::query_ops::{run_saved_query_nodes, RunSavedQueryInput};
use nodespace_core::services::query_service::core_queries::{
    core_query_templates, AWAITING_REVIEW_QUERY_ID, CORE_QUERY_IDS, IN_PROGRESS_QUERY_ID,
    READY_TASKS_QUERY_ID,
};
use nodespace_core::services::{CreateNodeParams, InsertPositionOwned, NodeService};
use serde_json::{json, Value};
use std::path::Path;
use std::sync::Arc;
use tempfile::TempDir;

async fn open(path: &Path) -> Result<Arc<NodeService>> {
    let mut store = Arc::new(SqliteStore::new(path.to_path_buf()).await?);
    Ok(Arc::new(NodeService::new(&mut store).await?))
}

async fn test_service() -> Result<(Arc<NodeService>, TempDir)> {
    let temp_dir = TempDir::new()?;
    let service = open(&temp_dir.path().join("test.db")).await?;
    Ok((service, temp_dir))
}

async fn create(service: &NodeService, node_type: &str, content: &str, props: Value) -> String {
    service
        .create_node(Node::new(node_type.to_string(), content.to_string(), props))
        .await
        .unwrap_or_else(|e| panic!("creating the {node_type} '{content}' failed: {e}"))
}

async fn create_child(
    service: &NodeService,
    parent: &str,
    node_type: &str,
    content: &str,
) -> String {
    service
        .create_node_with_parent(CreateNodeParams {
            id: None,
            node_type: node_type.to_string(),
            content: content.to_string(),
            parent_id: Some(parent.to_string()),
            position: InsertPositionOwned::End,
            properties: json!({}),
            lifecycle_status: None,
        })
        .await
        .unwrap_or_else(|e| panic!("placing '{content}' under {parent} failed: {e}"))
}

async fn link(service: &NodeService, from: &str, name: &str, to: &str) {
    service
        .create_relationship(from, name, to, json!({}))
        .await
        .unwrap_or_else(|e| panic!("linking {from} -{name}-> {to} failed: {e}"));
}

async fn set(service: &NodeService, id: &str, properties: Value) {
    let version = service.get_node(id).await.unwrap().unwrap().version;
    service
        .update_node(id, version, NodeUpdate::new().with_properties(properties))
        .await
        .unwrap_or_else(|e| panic!("updating {id} failed: {e}"));
}

/// An open task with one unchecked checklist item.
async fn ready_task(service: &NodeService, name: &str, props: Value) -> String {
    let id = create(service, "task", name, props).await;
    create_child(service, &id, "checkbox", "- [ ] It works").await;
    id
}

/// A plan in `plan_status`, linked to `task`. An approved plan is approved
/// the way a user approves one: against an approved spec with a criterion.
async fn plan_for(service: &NodeService, task: &str, approved: bool) -> String {
    let spec = create(service, "spec", "The spec", json!({})).await;
    create_child(service, &spec, "checkbox", "- [ ] It works").await;
    let plan = create(service, "plan", "The plan", json!({})).await;
    link(service, &plan, "spec", &spec).await;
    link(service, &plan, "tasks", task).await;
    if approved {
        set(service, &spec, json!({ "spec_status": "approved" })).await;
        set(service, &plan, json!({ "plan_status": "approved" })).await;
    }
    plan
}

/// The titles a saved query returns, in the order it returns them.
async fn run(service: &Arc<NodeService>, query: &str) -> Vec<String> {
    run_saved_query_nodes(
        service,
        RunSavedQueryInput {
            query: query.to_string(),
            filters: Vec::new(),
            limit: None,
            max_rows: None,
        },
    )
    .await
    .unwrap_or_else(|e| panic!("running '{query}' failed: {e}"))
    .nodes
    .into_iter()
    .map(|node| node.content)
    .collect()
}

#[tokio::test]
async fn every_database_is_seeded_with_the_six_queries_under_their_fixed_ids() -> Result<()> {
    let (service, _tmp) = test_service().await?;

    let mut titles = Vec::new();
    for id in CORE_QUERY_IDS {
        let node = service
            .get_node(id)
            .await?
            .unwrap_or_else(|| panic!("no seeded query with id {id}"));
        assert_eq!(node.node_type, "query");
        // A saved view is listed under the one type it targets.
        assert_ne!(QueryFields::from_node(&node)?.target_type, "*");
        titles.push(node.content);
    }
    assert_eq!(
        titles,
        [
            "Specs by status",
            "Plans by status",
            "Decisions by status",
            "Ready tasks",
            "In progress",
            "Awaiting review"
        ]
    );

    // They are what a listing of saved queries returns, and nothing else is.
    let mut listed: Vec<String> = service
        .query_nodes_by_type("query", false)
        .await?
        .into_iter()
        .map(|node| node.id)
        .collect();
    listed.sort();
    let mut expected: Vec<String> = CORE_QUERY_IDS.iter().map(|id| id.to_string()).collect();
    expected.sort();
    assert_eq!(listed, expected);
    Ok(())
}

/// Each condition of "Ready tasks" excludes a task on its own, and a task
/// with a checklist, no blocker and no plan is ready.
#[tokio::test]
async fn ready_tasks_keeps_an_open_unblocked_task_with_a_checklist_and_no_unapproved_plan(
) -> Result<()> {
    let (service, _tmp) = test_service().await?;

    // Ready: a checklist, no blocker, no plan.
    ready_task(&service, "ready", json!({})).await;

    // Excluded by status alone.
    let started = ready_task(&service, "started", json!({})).await;
    set(&service, &started, json!({ "status": "in_progress" })).await;

    // Excluded by having no checkbox child alone: a text child is not one.
    let no_checklist = create(&service, "task", "no checklist", json!({})).await;
    create_child(&service, &no_checklist, "text", "Notes").await;

    // Excluded by an unfinished blocker alone.
    let blocker = ready_task(&service, "blocker", json!({})).await;
    let blocked = ready_task(&service, "blocked", json!({})).await;
    link(&service, &blocker, "blocks", &blocked).await;

    // Excluded by a plan that is not approved alone.
    let drafted = ready_task(&service, "drafted", json!({})).await;
    plan_for(&service, &drafted, false).await;

    // Kept: its plan is approved.
    let planned = ready_task(&service, "planned", json!({})).await;
    plan_for(&service, &planned, true).await;

    // Kept: its only blocker is finished.
    let finished = ready_task(&service, "finished blocker", json!({})).await;
    let unblocked = ready_task(&service, "unblocked", json!({})).await;
    link(&service, &finished, "blocks", &unblocked).await;
    set(&service, &finished, json!({ "status": "cancelled" })).await;

    let mut ready = run(&service, "Ready tasks").await;
    ready.sort();
    assert_eq!(ready, ["blocker", "planned", "ready", "unblocked"]);
    Ok(())
}

#[tokio::test]
async fn ready_tasks_are_ordered_by_priority_then_by_creation_time() -> Result<()> {
    let (service, _tmp) = test_service().await?;

    ready_task(&service, "low", json!({ "priority": "low" })).await;
    ready_task(&service, "first high", json!({ "priority": "high" })).await;
    ready_task(&service, "highest", json!({ "priority": "highest" })).await;
    ready_task(&service, "second high", json!({ "priority": "high" })).await;

    assert_eq!(
        run(&service, READY_TASKS_QUERY_ID).await,
        ["highest", "first high", "second high", "low"]
    );
    Ok(())
}

/// A task is in exactly the queue of the stage it is in.
#[tokio::test]
async fn a_task_moves_from_queue_to_queue_with_its_status() -> Result<()> {
    let (service, _tmp) = test_service().await?;
    let task = ready_task(&service, "the task", json!({})).await;
    let queues = [
        READY_TASKS_QUERY_ID,
        IN_PROGRESS_QUERY_ID,
        AWAITING_REVIEW_QUERY_ID,
    ];

    for (status, queue) in [
        ("open", Some(READY_TASKS_QUERY_ID)),
        ("in_progress", Some(IN_PROGRESS_QUERY_ID)),
        ("in_review", Some(AWAITING_REVIEW_QUERY_ID)),
        ("cancelled", None),
    ] {
        set(&service, &task, json!({ "status": status })).await;
        for candidate in queues {
            let listed = !run(&service, candidate).await.is_empty();
            assert_eq!(
                listed,
                Some(candidate) == queue,
                "a task that is {status} and the queue {candidate}"
            );
        }
    }
    Ok(())
}

/// ADR-072: a user's edit to a seeded query survives a reopen, and a reset
/// restores what ships.
#[tokio::test]
async fn an_edited_query_survives_a_reopen_and_a_reset_restores_it() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let path = temp_dir.path().join("test.db");
    let limit = |node: &Node| QueryFields::from_node(node).unwrap().limit;

    {
        let service = open(&path).await?;
        set(&service, READY_TASKS_QUERY_ID, json!({ "limit": 3 })).await;
    }

    let service = open(&path).await?;
    let edited = service.get_node(READY_TASKS_QUERY_ID).await?.unwrap();
    assert_eq!(limit(&edited), Some(3));

    let template = core_query_templates()
        .into_iter()
        .find(|template| template.id == READY_TASKS_QUERY_ID)
        .unwrap();
    service
        .reset_seed_node(&prepare_nodes_from_template(&template)?, true, false)
        .await?;
    let reset = service.get_node(READY_TASKS_QUERY_ID).await?.unwrap();
    assert_eq!(limit(&reset), None);
    assert_eq!(
        QueryFields::from_node(&reset)?.filters,
        QueryFields::from_properties(&template.root_properties)?.filters
    );
    Ok(())
}
