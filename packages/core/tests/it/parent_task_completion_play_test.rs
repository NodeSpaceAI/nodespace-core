//! The core parent-task completion rollup Play, end to end (ADR-079).
//!
//! These drive the *seeded* Play — nothing here installs a rule — so they also
//! assert that it is shipped, active, and correctly authored against the real
//! `task` schema, not just that the rule shape works in isolation.

use anyhow::Result;
use nodespace_core::db::SqliteStore;
use nodespace_core::models::Node;
use nodespace_core::playbook::core_plays::{CORE_PLAY_IDS, PARENT_TASK_COMPLETION_PLAY_ID};
use nodespace_core::playbook::PlaybookEngine;
use nodespace_core::services::NodeService;
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;
use tempfile::TempDir;
use tokio::sync::watch;

async fn create_test_service() -> Result<(Arc<NodeService>, TempDir)> {
    let temp_dir = TempDir::new()?;
    let db_path = temp_dir.path().join("test.db");
    let mut store = Arc::new(SqliteStore::new(db_path).await?);
    let service = Arc::new(NodeService::new(&mut store).await?);
    Ok((service, temp_dir))
}

async fn spawn_engine(
    service: &Arc<NodeService>,
) -> (watch::Sender<bool>, tokio::task::JoinHandle<Result<()>>) {
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let engine = Arc::new(PlaybookEngine::new(Arc::clone(service)));
    service.set_playbook_lifecycle(engine.lifecycle().clone());
    let task = tokio::spawn(async move {
        engine.start(shutdown_rx).await?;
        Ok(())
    });
    tokio::time::sleep(Duration::from_millis(50)).await;
    (shutdown_tx, task)
}

async fn shutdown_engine(tx: watch::Sender<bool>, task: tokio::task::JoinHandle<Result<()>>) {
    let _ = tx.send(true);
    let _ = tokio::time::timeout(Duration::from_secs(5), task).await;
}

async fn wait_until<F, Fut>(mut check: F) -> bool
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    for _ in 0..80 {
        if check().await {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    false
}

async fn task_node(service: &NodeService, status: &str) -> Result<String> {
    let node = Node::new(
        "task".to_string(),
        "a task".to_string(),
        // The light lane: the test moves tasks to `in_progress`, and it is
        // the roll-up it exercises, not the spec rule.
        json!({ "status": status, "requires_spec": false }),
    );
    Ok(service.create_node(node).await?)
}

async fn status_of(service: &NodeService, id: &str) -> Option<String> {
    let node = service.get_node(id).await.ok()??;
    node.properties
        .get("task")
        .and_then(|t| t.get("status"))
        .or_else(|| node.properties.get("status"))
        .and_then(|v| v.as_str())
        .map(str::to_string)
}

async fn set_status(service: &NodeService, id: &str, status: &str) -> Result<()> {
    let current = service.get_node(id).await?.expect("node should exist");
    service
        .update_node(
            id,
            current.version,
            nodespace_core::models::NodeUpdate::default()
                .with_properties(json!({ "status": status })),
        )
        .await?;
    Ok(())
}

/// A child of `parent` that is not a task: a checkbox or a note.
async fn child_node(
    service: &NodeService,
    parent: &str,
    node_type: &str,
    content: &str,
) -> Result<String> {
    let id = service
        .create_node(Node::new(
            node_type.to_string(),
            content.to_string(),
            json!({}),
        ))
        .await?;
    service
        .create_relationship(parent, "has_child", &id, json!({}))
        .await?;
    Ok(id)
}

async fn set_content(service: &NodeService, id: &str, content: &str) -> Result<()> {
    let current = service.get_node(id).await?.expect("node should exist");
    service
        .update_node(
            id,
            current.version,
            nodespace_core::models::NodeUpdate::default().with_content(content.to_string()),
        )
        .await?;
    Ok(())
}

/// A parent task with one open sub-task under it.
async fn parent_with_sub_task(service: &NodeService) -> Result<(String, String)> {
    let parent = task_node(service, "open").await?;
    let sub_task = task_node(service, "open").await?;
    service
        .create_relationship(&parent, "has_child", &sub_task, json!({}))
        .await?;
    Ok((parent, sub_task))
}

/// Whether the roll-up Play is enabled and not suspended.
async fn roll_up_is_running(service: &NodeService) -> Result<bool> {
    let play = service
        .get_node(PARENT_TASK_COMPLETION_PLAY_ID)
        .await?
        .expect("seeded");
    let play = &play.properties["play"];
    Ok(play["enabled"] == json!(true) && play.get("suspended_reason").is_none_or(|v| v.is_null()))
}

async fn wait_for_status(service: &Arc<NodeService>, id: &str, want: &str) -> bool {
    let want = want.to_string();
    wait_until(|| {
        let service = Arc::clone(service);
        let id = id.to_string();
        let want = want.clone();
        async move { status_of(&service, &id).await.as_deref() == Some(want.as_str()) }
    })
    .await
}

/// The Play must ship installed and active — not merely be definable.
#[tokio::test]
async fn the_rollup_play_is_seeded_and_active() -> Result<()> {
    let (service, _tmp) = create_test_service().await?;

    let play = service
        .get_node(PARENT_TASK_COMPLETION_PLAY_ID)
        .await?
        .expect("the core rollup Play must be seeded into every database");

    assert_eq!(play.node_type, "play");
    assert_eq!(
        play.lifecycle_status, "active",
        "a seeded core Play must be active, or it never fires"
    );
    assert!(
        play.properties.get("_seed").is_some()
            || play
                .properties
                .get("play")
                .and_then(|p| p.get("_seed"))
                .is_some(),
        "must carry the _seed marker so it can be reset (ADR-060 §8)"
    );
    Ok(())
}

/// Seeding reconciles per id, so re-opening a database must not duplicate a
/// core Play.
#[tokio::test]
async fn reopening_a_database_does_not_duplicate_the_play() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let db_path = temp_dir.path().join("test.db");
    let mut expected: Vec<&str> = CORE_PLAY_IDS.to_vec();
    expected.sort_unstable();
    assert!(expected.contains(&PARENT_TASK_COMPLETION_PLAY_ID));

    for _ in 0..2 {
        let mut store = Arc::new(SqliteStore::new(db_path.clone()).await?);
        let service = Arc::new(NodeService::new(&mut store).await?);
        let plays = service.query_nodes_by_type("play", true).await?;
        let mut ids: Vec<&str> = plays.iter().map(|p| p.id.as_str()).collect();
        ids.sort_unstable();
        assert_eq!(
            ids, expected,
            "core Plays must reconcile per id, not re-seed on every open"
        );
    }
    Ok(())
}

/// The headline behavior: the last child completing completes the parent.
#[tokio::test]
async fn completing_the_last_child_completes_the_parent() -> Result<()> {
    let (service, _tmp) = create_test_service().await?;
    let (tx, engine_task) = spawn_engine(&service).await;

    let parent = task_node(&service, "open").await?;
    let child_a = task_node(&service, "open").await?;
    let child_b = task_node(&service, "open").await?;
    service
        .create_relationship(&parent, "has_child", &child_a, json!({}))
        .await?;
    service
        .create_relationship(&parent, "has_child", &child_b, json!({}))
        .await?;

    // One of two children done — the parent must stay open.
    set_status(&service, &child_a, "done").await?;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        status_of(&service, &parent).await.as_deref(),
        Some("open"),
        "the parent must not complete while a child is still open"
    );

    // The last child completes — now the parent rolls up.
    set_status(&service, &child_b, "done").await?;
    assert!(
        wait_for_status(&service, &parent, "done").await,
        "completing the last child must complete the parent"
    );

    shutdown_engine(tx, engine_task).await;
    Ok(())
}

/// ADR-079 §1: `cancelled` is terminal for rollup — no work remains under the
/// parent, so it completes (as `done`, never `cancelled`).
#[tokio::test]
async fn a_cancelled_sibling_counts_as_finished() -> Result<()> {
    let (service, _tmp) = create_test_service().await?;
    let (tx, engine_task) = spawn_engine(&service).await;

    let parent = task_node(&service, "open").await?;
    let done_child = task_node(&service, "open").await?;
    let cancelled_child = task_node(&service, "open").await?;
    service
        .create_relationship(&parent, "has_child", &done_child, json!({}))
        .await?;
    service
        .create_relationship(&parent, "has_child", &cancelled_child, json!({}))
        .await?;

    set_status(&service, &cancelled_child, "cancelled").await?;
    set_status(&service, &done_child, "done").await?;

    assert!(
        wait_for_status(&service, &parent, "done").await,
        "a cancelled child leaves no work outstanding, so the parent completes"
    );
    assert_eq!(
        status_of(&service, &parent).await.as_deref(),
        Some("done"),
        "the parent completes as done, never as cancelled"
    );

    shutdown_engine(tx, engine_task).await;
    Ok(())
}

/// ADR-079 §3: a task with no children must never
/// auto-complete itself. Guaranteed by empty-collection-is-false, including
/// under `.all()` — the opposite of CEL's usual vacuous truth.
///
/// Paired with a positive control in the same test: a sibling hierarchy that
/// DOES complete. Without it this would pass just as well if the Play were
/// disabled, misseeded, or never fired at all — the failure mode a negative
/// assertion is least able to distinguish on its own.
#[tokio::test]
async fn a_childless_task_never_auto_completes() -> Result<()> {
    let (service, _tmp) = create_test_service().await?;
    let (tx, engine_task) = spawn_engine(&service).await;

    let lonely = task_node(&service, "open").await?;

    // Positive control: a parent whose only child completes.
    let control_parent = task_node(&service, "open").await?;
    let control_child = task_node(&service, "open").await?;
    service
        .create_relationship(&control_parent, "has_child", &control_child, json!({}))
        .await?;

    set_status(&service, &lonely, "in_progress").await?;
    set_status(&service, &control_child, "done").await?;

    assert!(
        wait_for_status(&service, &control_parent, "done").await,
        "positive control must complete — otherwise this test proves nothing \
         about the childless case"
    );

    assert_eq!(
        status_of(&service, &lonely).await.as_deref(),
        Some("in_progress"),
        "a task with no children must not complete itself"
    );

    shutdown_engine(tx, engine_task).await;
    Ok(())
}

/// The rollup's single-parent assumption is enforced at write time, not merely
/// assumed: a second `has_child` edge onto the same node is rejected.
///
/// Before this was enforced, a built-in relationship skipped the declared-
/// cardinality check (it has no `SchemaRelationship` to carry it), so the CLI's
/// `relationship create --type has_child` and the agent's `create_relationship`
/// tool could both produce a two-parent node. Nothing errored — `get_parent`'s
/// `LIMIT 1` silently hid one parent, and this Play then skipped the node with
/// no signal.
#[tokio::test]
async fn a_second_parent_edge_is_rejected() -> Result<()> {
    let (service, _tmp) = create_test_service().await?;

    let parent_a = task_node(&service, "open").await?;
    let parent_b = task_node(&service, "open").await?;
    let child = task_node(&service, "open").await?;

    service
        .create_relationship(&parent_a, "has_child", &child, json!({}))
        .await?;

    let err = service
        .create_relationship(&parent_b, "has_child", &child, json!({}))
        .await
        .expect_err("a second has_child parent must be rejected");
    let msg = err.to_string();
    assert!(
        msg.contains("already has parent"),
        "the error must name the conflict, got: {msg}"
    );

    // The first parent still owns the edge — a rejected write changes nothing.
    let parent = service
        .get_parent(&child)
        .await?
        .expect("the original parent edge must survive");
    assert_eq!(parent.id, parent_a);

    // Re-asserting the SAME edge is not a second parent, so it is allowed —
    // callers that re-attach an existing child must not start failing.
    service
        .create_relationship(&parent_a, "has_child", &child, json!({}))
        .await?;

    Ok(())
}

/// Reparenting must keep working: it is delete-then-insert inside one
/// transaction at the store layer, below `create_relationship`, so the
/// single-parent guard never sees it.
///
/// Worth pinning explicitly — a guard that rejects a second parent is exactly
/// the shape that breaks a move implemented as "attach new, detach old", and
/// nothing else in this file would catch that regression.
#[tokio::test]
async fn reparenting_still_works_with_the_single_parent_guard() -> Result<()> {
    let (service, _tmp) = create_test_service().await?;

    let parent_a = task_node(&service, "open").await?;
    let parent_b = task_node(&service, "open").await?;
    let child = task_node(&service, "open").await?;
    service
        .create_relationship(&parent_a, "has_child", &child, json!({}))
        .await?;

    service
        .move_node_unchecked(
            &child,
            Some(&parent_b),
            nodespace_core::services::InsertPosition::End,
        )
        .await?;

    let parent = service
        .get_parent(&child)
        .await?
        .expect("the moved node must have a parent");
    assert_eq!(
        parent.id, parent_b,
        "the move must re-point the parent edge"
    );

    // And exactly one edge survives — a move that left the old edge behind
    // would produce the two-parent state the guard exists to prevent.
    let children_of_a = service.get_children(&parent_a).await?;
    assert!(
        children_of_a.is_empty(),
        "the previous parent must no longer claim the child"
    );

    Ok(())
}

/// ADR-079 §4: the rollup climbs the ancestor spine, one level per firing —
/// the Play's own write to the parent re-triggers it with the parent now in
/// the child position.
#[tokio::test]
async fn completion_cascades_to_the_grandparent() -> Result<()> {
    let (service, _tmp) = create_test_service().await?;
    let (tx, engine_task) = spawn_engine(&service).await;

    let grandparent = task_node(&service, "open").await?;
    let parent = task_node(&service, "open").await?;
    let leaf = task_node(&service, "open").await?;
    service
        .create_relationship(&grandparent, "has_child", &parent, json!({}))
        .await?;
    service
        .create_relationship(&parent, "has_child", &leaf, json!({}))
        .await?;

    set_status(&service, &leaf, "done").await?;

    assert!(
        wait_for_status(&service, &parent, "done").await,
        "the immediate parent must complete"
    );
    assert!(
        wait_for_status(&service, &grandparent, "done").await,
        "completion must cascade to the grandparent"
    );

    shutdown_engine(tx, engine_task).await;
    Ok(())
}

/// ADR-079 §2, end to end: the shipped Play — authored against `task`, with no
/// knowledge that `bug` exists — must complete a parent whose children are
/// `bug` nodes carrying `bug`'s own extended status vocabulary.
///
/// This is the acceptance criterion the scope unit tests cover only in pieces:
/// they prove `maps_to` resolution works, this proves the *seeded* Play
/// actually benefits from it across a relationship walk. `shipped` is
/// `bug`-only and maps to `done` at `task` scope, so a `task`-scoped condition
/// asking for `done` must match it.
#[tokio::test]
async fn the_play_completes_a_parent_whose_children_are_an_extending_subtype() -> Result<()> {
    let (service, _tmp) = create_test_service().await?;

    nodespace_core::schema::handle_create_schema(
        &service,
        json!({ "name": "Bug", "extends": "task", "fields": [] }),
    )
    .await
    .expect("creating a task-extending schema should succeed");

    nodespace_core::schema::handle_update_schema(
        &service,
        json!({
            "schema_id": "bug",
            "add_field_values": [{
                "field": "status",
                "values": [{ "value": "shipped", "label": "Shipped", "mapsTo": "done" }]
            }],
            // Additive, so the impact guard does not ask — asserted directly
            // by the additive/destructive split. `force` is deliberately NOT
            // passed here: if extending an inherited enum ever starts
            // demanding it again, this test is where that regresses.
        }),
    )
    .await
    .expect("extending an inherited enum should succeed without force");

    let (tx, engine_task) = spawn_engine(&service).await;

    let parent = task_node(&service, "open").await?;
    let bug = service
        .create_node(Node::new(
            "bug".to_string(),
            "a bug".to_string(),
            json!({ "status": "open" }),
        ))
        .await?;
    service
        .create_relationship(&parent, "has_child", &bug, json!({}))
        .await?;

    // `shipped` is vocabulary `task` has never heard of; it must read as `done`.
    set_status(&service, &bug, "shipped").await?;

    assert!(
        wait_for_status(&service, &parent, "done").await,
        "a subtype child's extended status must resolve at task scope through \
         maps_to, so the base-scoped Play completes the parent unchanged"
    );

    shutdown_engine(tx, engine_task).await;
    Ok(())
}

/// A parent with one still-open child among several must not complete — the
/// guard against an over-eager `.all()`.
#[tokio::test]
async fn a_parent_with_one_open_child_stays_open() -> Result<()> {
    let (service, _tmp) = create_test_service().await?;
    let (tx, engine_task) = spawn_engine(&service).await;

    let parent = task_node(&service, "open").await?;
    let finished = task_node(&service, "open").await?;
    let still_open = task_node(&service, "open").await?;
    service
        .create_relationship(&parent, "has_child", &finished, json!({}))
        .await?;
    service
        .create_relationship(&parent, "has_child", &still_open, json!({}))
        .await?;

    set_status(&service, &finished, "done").await?;
    tokio::time::sleep(Duration::from_millis(400)).await;

    assert_eq!(
        status_of(&service, &parent).await.as_deref(),
        Some("open"),
        "one outstanding child must keep the parent open"
    );

    shutdown_engine(tx, engine_task).await;
    Ok(())
}

/// A parent with a checklist of its own is completed by the roll-up when its
/// last sub-task finishes, provided the checklist is complete (ADR-079 §8).
#[tokio::test]
async fn a_parent_with_a_checked_checklist_completes_with_its_last_sub_task() -> Result<()> {
    let (service, _tmp) = create_test_service().await?;
    let (tx, engine_task) = spawn_engine(&service).await;

    let (parent, sub_task) = parent_with_sub_task(&service).await?;
    let other = task_node(&service, "open").await?;
    service
        .create_relationship(&parent, "has_child", &other, json!({}))
        .await?;
    child_node(&service, &parent, "checkbox", "- [x] It works").await?;
    child_node(&service, &parent, "checkbox", "- [x] It is documented").await?;

    set_status(&service, &sub_task, "done").await?;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        status_of(&service, &parent).await.as_deref(),
        Some("open"),
        "a sub-task is still open"
    );

    set_status(&service, &other, "cancelled").await?;
    assert!(
        wait_for_status(&service, &parent, "done").await,
        "a checked checklist must not stop the roll-up"
    );

    shutdown_engine(tx, engine_task).await;
    Ok(())
}

/// The criteria rule is the judge of the checklist: a parent with an
/// unchecked item is left open by it, the roll-up is not suspended, and it
/// goes on completing other parents.
#[tokio::test]
async fn a_parent_with_an_unchecked_item_stays_open_and_the_roll_up_keeps_running() -> Result<()> {
    let (service, _tmp) = create_test_service().await?;
    let (tx, engine_task) = spawn_engine(&service).await;

    let (parent, sub_task) = parent_with_sub_task(&service).await?;
    child_node(&service, &parent, "checkbox", "- [x] It works").await?;
    child_node(&service, &parent, "checkbox", "- [ ] Not met yet").await?;
    set_status(&service, &sub_task, "done").await?;

    // Completed after the refusal: only a Play still running rolls it up.
    let (control_parent, control_sub_task) = parent_with_sub_task(&service).await?;
    set_status(&service, &control_sub_task, "done").await?;
    assert!(
        wait_for_status(&service, &control_parent, "done").await,
        "the roll-up must go on completing other parents"
    );

    assert_eq!(
        status_of(&service, &parent).await.as_deref(),
        Some("open"),
        "an unchecked item keeps the parent open"
    );
    assert!(
        roll_up_is_running(&service).await?,
        "a refused roll-up must not suspend the Play"
    );

    shutdown_engine(tx, engine_task).await;
    Ok(())
}

/// ADR-079 §8: ticking the last item after the sub-tasks have finished does
/// not complete the parent. A checklist gates completion and never causes
/// it, so the parent is finished by hand, which the criteria rule now allows.
#[tokio::test]
async fn a_checklist_finished_after_the_sub_tasks_leaves_the_parent_to_be_finished_by_hand(
) -> Result<()> {
    let (service, _tmp) = create_test_service().await?;
    let (tx, engine_task) = spawn_engine(&service).await;

    let (parent, sub_task) = parent_with_sub_task(&service).await?;
    let item = child_node(&service, &parent, "checkbox", "- [ ] Not met yet").await?;
    set_status(&service, &sub_task, "done").await?;

    // Positive control, so the check below follows a roll-up that has run.
    let (control_parent, control_sub_task) = parent_with_sub_task(&service).await?;
    set_status(&service, &control_sub_task, "done").await?;
    assert!(wait_for_status(&service, &control_parent, "done").await);

    set_content(&service, &item, "- [x] Met").await?;
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert_eq!(
        status_of(&service, &parent).await.as_deref(),
        Some("open"),
        "ticking an item is not a task status change, so nothing re-evaluates the parent"
    );

    set_status(&service, &parent, "done").await?;
    assert_eq!(status_of(&service, &parent).await.as_deref(), Some("done"));
    assert!(roll_up_is_running(&service).await?);

    shutdown_engine(tx, engine_task).await;
    Ok(())
}

/// A note, or any other child that is not a task, is not asked for a status.
#[tokio::test]
async fn a_child_that_is_not_a_task_does_not_stop_the_roll_up() -> Result<()> {
    let (service, _tmp) = create_test_service().await?;
    let (tx, engine_task) = spawn_engine(&service).await;

    let (parent, sub_task) = parent_with_sub_task(&service).await?;
    child_node(&service, &parent, "text", "A note about the work").await?;
    child_node(&service, &parent, "header", "## Context").await?;

    set_status(&service, &sub_task, "done").await?;
    assert!(
        wait_for_status(&service, &parent, "done").await,
        "a note under the parent must not stop the roll-up"
    );

    shutdown_engine(tx, engine_task).await;
    Ok(())
}

/// A task whose only children are checkboxes and notes has no sub-task to
/// finish, so nothing completes it: not a change to its own status, and not
/// its checklist being completed.
#[tokio::test]
async fn a_task_with_no_task_children_never_auto_completes() -> Result<()> {
    let (service, _tmp) = create_test_service().await?;
    let (tx, engine_task) = spawn_engine(&service).await;

    let task = task_node(&service, "open").await?;
    let item = child_node(&service, &task, "checkbox", "- [ ] Not met yet").await?;
    child_node(&service, &task, "text", "A note").await?;

    set_status(&service, &task, "in_progress").await?;
    set_content(&service, &item, "- [x] Met").await?;

    let (control_parent, control_sub_task) = parent_with_sub_task(&service).await?;
    set_status(&service, &control_sub_task, "done").await?;
    assert!(wait_for_status(&service, &control_parent, "done").await);

    assert_eq!(
        status_of(&service, &task).await.as_deref(),
        Some("in_progress"),
        "a task with no sub-tasks must not complete itself"
    );

    shutdown_engine(tx, engine_task).await;
    Ok(())
}

/// Finished tasks that sit under a node that is not a task have no parent
/// task to complete: the roll-up leaves that node alone and keeps running.
#[tokio::test]
async fn finished_tasks_under_a_node_that_is_not_a_task_are_left_alone() -> Result<()> {
    let (service, _tmp) = create_test_service().await?;
    let (tx, engine_task) = spawn_engine(&service).await;

    let page = service
        .create_node(Node::new(
            "text".to_string(),
            "A page of tasks".to_string(),
            json!({}),
        ))
        .await?;
    let task = task_node(&service, "open").await?;
    service
        .create_relationship(&page, "has_child", &task, json!({}))
        .await?;
    child_node(&service, &page, "text", "A note beside the task").await?;
    let before = service.get_node(&page).await?.expect("the page");
    set_status(&service, &task, "done").await?;

    let (control_parent, control_sub_task) = parent_with_sub_task(&service).await?;
    set_status(&service, &control_sub_task, "done").await?;
    assert!(wait_for_status(&service, &control_parent, "done").await);

    let after = service.get_node(&page).await?.expect("the page");
    assert_eq!(
        after.properties, before.properties,
        "a node that is not a task must be given no status"
    );
    assert_eq!(after.version, before.version);
    assert!(roll_up_is_running(&service).await?);

    shutdown_engine(tx, engine_task).await;
    Ok(())
}

/// A project has a `status` of its own, in its own vocabulary, and tasks are
/// outlined under one. It is not a task: the roll-up writes it nothing, and
/// is not suspended by a write a project would refuse.
#[tokio::test]
async fn finished_tasks_under_a_project_leave_the_project_alone() -> Result<()> {
    let (service, _tmp) = create_test_service().await?;
    let (tx, engine_task) = spawn_engine(&service).await;

    let project = service
        .create_node(Node::new(
            "project".to_string(),
            "A project".to_string(),
            json!({ "status": "active" }),
        ))
        .await?;
    let task = task_node(&service, "open").await?;
    service
        .create_relationship(&project, "has_child", &task, json!({}))
        .await?;
    child_node(&service, &project, "text", "A note beside the task").await?;
    let before = service.get_node(&project).await?.expect("the project");
    set_status(&service, &task, "done").await?;

    let (control_parent, control_sub_task) = parent_with_sub_task(&service).await?;
    set_status(&service, &control_sub_task, "done").await?;
    assert!(
        wait_for_status(&service, &control_parent, "done").await,
        "the roll-up must still be running after tasks under a project finish"
    );

    let after = service.get_node(&project).await?.expect("the project");
    assert_eq!(after.properties, before.properties);
    assert_eq!(after.version, before.version);
    assert!(roll_up_is_running(&service).await?);

    shutdown_engine(tx, engine_task).await;
    Ok(())
}

/// A child of another type that has a `status` of its own is not a sub-task:
/// a project under a task neither finishes the parent nor holds it open.
#[tokio::test]
async fn a_child_of_another_type_with_its_own_status_is_not_a_sub_task() -> Result<()> {
    let (service, _tmp) = create_test_service().await?;
    let (tx, engine_task) = spawn_engine(&service).await;

    let (parent, sub_task) = parent_with_sub_task(&service).await?;
    let project = service
        .create_node(Node::new(
            "project".to_string(),
            "A project".to_string(),
            json!({ "status": "active" }),
        ))
        .await?;
    service
        .create_relationship(&parent, "has_child", &project, json!({}))
        .await?;

    set_status(&service, &sub_task, "done").await?;
    assert!(
        wait_for_status(&service, &parent, "done").await,
        "an active project under the parent must not hold it open"
    );

    shutdown_engine(tx, engine_task).await;
    Ok(())
}

/// ADR-079 §2: a sub-task whose status cannot be read is not finished. It is
/// a task, so it is not passed over like a note: it holds the parent open.
#[tokio::test]
async fn a_sub_task_with_no_status_holds_the_parent_open() -> Result<()> {
    let (service, _tmp) = create_test_service().await?;
    let (tx, engine_task) = spawn_engine(&service).await;

    let (parent, sub_task) = parent_with_sub_task(&service).await?;
    let cleared = task_node(&service, "open").await?;
    service
        .create_relationship(&parent, "has_child", &cleared, json!({}))
        .await?;
    let current = service.get_node(&cleared).await?.expect("the sub-task");
    service
        .update_node(
            &cleared,
            current.version,
            nodespace_core::models::NodeUpdate::default()
                .with_properties(json!({ "status": null })),
        )
        .await?;
    assert_eq!(
        status_of(&service, &cleared).await,
        None,
        "precondition: the sub-task reads no status"
    );

    set_status(&service, &sub_task, "done").await?;

    let (control_parent, control_sub_task) = parent_with_sub_task(&service).await?;
    set_status(&service, &control_sub_task, "done").await?;
    assert!(wait_for_status(&service, &control_parent, "done").await);

    assert_eq!(
        status_of(&service, &parent).await.as_deref(),
        Some("open"),
        "a sub-task with no status must hold the parent open"
    );
    assert!(roll_up_is_running(&service).await?);

    shutdown_engine(tx, engine_task).await;
    Ok(())
}

/// A subtype of `task` is a task on both sides of the roll-up: as the parent
/// that is completed, and as a sub-task counted beside checkboxes and notes.
#[tokio::test]
async fn a_subtype_of_task_counts_as_parent_and_as_sub_task() -> Result<()> {
    let (service, _tmp) = create_test_service().await?;
    nodespace_core::schema::handle_create_schema(
        &service,
        json!({ "name": "Bug", "extends": "task", "fields": [] }),
    )
    .await
    .expect("creating a task-extending schema should succeed");
    let (tx, engine_task) = spawn_engine(&service).await;

    let bug = |title: &str| {
        Node::new(
            "bug".to_string(),
            title.to_string(),
            json!({ "status": "open" }),
        )
    };
    let parent = service.create_node(bug("a parent bug")).await?;
    let finished = service.create_node(bug("a finished bug")).await?;
    let open = service.create_node(bug("an open bug")).await?;
    for child in [&finished, &open] {
        service
            .create_relationship(&parent, "has_child", child, json!({}))
            .await?;
    }
    child_node(&service, &parent, "checkbox", "- [x] It works").await?;
    child_node(&service, &parent, "text", "A note").await?;

    set_status(&service, &finished, "done").await?;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        status_of(&service, &parent).await.as_deref(),
        Some("open"),
        "an open subtype sub-task must keep the parent open"
    );

    set_status(&service, &open, "done").await?;
    assert!(
        wait_for_status(&service, &parent, "done").await,
        "a subtype parent must complete once its subtype sub-tasks finish"
    );

    shutdown_engine(tx, engine_task).await;
    Ok(())
}
