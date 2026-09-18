//! The core parent-task completion rollup Play, end to end (ADR-079).
//!
//! These drive the *seeded* Play — nothing here installs a rule — so they also
//! assert that it is shipped, active, and correctly authored against the real
//! `task` schema, not just that the rule shape works in isolation.

use anyhow::Result;
use nodespace_core::db::SqliteStore;
use nodespace_core::models::Node;
use nodespace_core::playbook::core_plays::PARENT_TASK_COMPLETION_PLAY_ID;
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
        json!({ "status": status }),
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

/// Seeding reconciles per id, so re-opening a database must not duplicate it.
#[tokio::test]
async fn reopening_a_database_does_not_duplicate_the_play() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let db_path = temp_dir.path().join("test.db");

    for _ in 0..2 {
        let mut store = Arc::new(SqliteStore::new(db_path.clone()).await?);
        let service = Arc::new(NodeService::new(&mut store).await?);
        let plays = service.query_nodes_by_type("play", None).await?;
        assert_eq!(
            plays.len(),
            1,
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
