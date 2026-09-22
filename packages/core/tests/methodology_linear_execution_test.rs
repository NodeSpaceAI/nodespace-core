//! What the Linear recipe's Plays actually DO, against a running engine.
//!
//! The install test proves every Play saves. That is a weaker property than it
//! looks: a rule can validate, activate, and then do nothing, or do the wrong
//! thing, with every structural assertion still green. Three separate bugs in
//! this recipe had exactly that shape — a reassignment that re-added each task
//! to the cycle it was already in, a rollover that moved completed work, and a
//! condition over a relationship that resolved to an empty set because the
//! runtime resolver did not walk the `extends` chain the validator did.
//!
//! None of those were visible to a test that asserts on rule JSON. They are
//! visible here, because these tests run the engine and then look at the graph.

use anyhow::Result;
use nodespace_core::db::SqliteStore;
use nodespace_core::methodology::{install_recipe, recipe_by_id};
use nodespace_core::models::Node;
use nodespace_core::services::NodeService;
use nodespace_core::PlaybookEngine;
use std::sync::Arc;
use std::time::Duration;
use tempfile::TempDir;
use tokio::sync::watch;

async fn test_service() -> Result<(Arc<NodeService>, TempDir)> {
    let temp_dir = TempDir::new()?;
    let db_path = temp_dir.path().join("test.db");
    let mut store = Arc::new(SqliteStore::new(db_path).await?);
    let service = Arc::new(NodeService::new(&mut store).await?);
    Ok((service, temp_dir))
}

/// Install the recipe and start the engine, so invariant rules are live on the
/// write path.
async fn service_with_recipe() -> Result<(
    Arc<NodeService>,
    TempDir,
    watch::Sender<bool>,
    tokio::task::JoinHandle<Result<()>>,
)> {
    let (service, tmp) = test_service().await?;
    let recipe = recipe_by_id("linear").expect("linear recipe ships");
    let report = install_recipe(&service, &recipe).await;
    assert!(report.success, "install failed: {:?}", report.failure());

    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let engine = Arc::new(PlaybookEngine::new(Arc::clone(&service)));
    service.set_playbook_lifecycle(engine.lifecycle().clone());
    let task = tokio::spawn(async move { engine.start(shutdown_rx).await });
    tokio::time::sleep(Duration::from_millis(80)).await;

    Ok((service, tmp, shutdown_tx, task))
}

async fn shutdown(
    tx: watch::Sender<bool>,
    task: tokio::task::JoinHandle<Result<()>>,
) -> Result<()> {
    let _ = tx.send(true);
    let _ = tokio::time::timeout(Duration::from_secs(2), task).await;
    Ok(())
}

/// An `issue` nested under `parent` — a sub-issue is an ordinary child node,
/// so hierarchy is an edge set at create time, not a field on the node.
fn child_params(
    parent: &str,
    content: &str,
    status: &str,
) -> nodespace_core::services::CreateNodeParams {
    nodespace_core::services::CreateNodeParams {
        id: None,
        node_type: "issue".to_string(),
        content: content.to_string(),
        parent_id: Some(parent.to_string()),
        position: nodespace_core::services::InsertPositionOwned::End,
        properties: serde_json::json!({ "status": status }),
        lifecycle_status: None,
    }
}

/// The blocker gate must actually reject.
///
/// This is the test C3 needed: the Play installs and activates whether or not
/// `node.blocked_by` resolves to anything, so "it saved" proves nothing. Only
/// attempting the write it is supposed to veto distinguishes a working gate
/// from a silently empty condition.
#[tokio::test]
async fn blocker_gate_rejects_starting_an_issue_with_an_open_blocker() -> Result<()> {
    let (service, _tmp, tx, task) = service_with_recipe().await?;

    let blocker = service
        .create_node(Node::new(
            "issue".to_string(),
            "The blocker".to_string(),
            serde_json::json!({ "status": "open" }),
        ))
        .await?;
    let blocked = service
        .create_node(Node::new(
            "issue".to_string(),
            "The blocked issue".to_string(),
            serde_json::json!({ "status": "open" }),
        ))
        .await?;

    // blocker -[blocks]-> blocked, so `blocked.blocked_by` reaches `blocker`.
    // The relationship is declared on `task` and inherited by `issue`.
    service
        .create_relationship(&blocker, "blocks", &blocked, serde_json::json!({}))
        .await?;

    let node = service.get_node(&blocked).await?.expect("blocked exists");
    let result = service
        .update_node(
            &blocked,
            node.version,
            nodespace_core::models::NodeUpdate::default()
                .with_properties(serde_json::json!({ "status": "in_progress" })),
        )
        .await;

    assert!(
        result.is_err(),
        "starting an issue whose blocker is still open must be rejected — \
         if this passes, the gate's condition resolved to an empty set and the \
         Play is a silent no-op"
    );

    // And the write really did not land.
    let after = service.get_node(&blocked).await?.expect("still exists");
    let status = after
        .properties
        .get("issue")
        .and_then(|b| b.get("status"))
        .or_else(|| after.properties.get("status"))
        .and_then(|v| v.as_str());
    assert_ne!(
        status,
        Some("in_progress"),
        "the rejected write must not persist"
    );

    shutdown(tx, task).await
}

/// The same gate must let a legitimate start through — otherwise "rejects
/// everything" would pass the test above.
#[tokio::test]
async fn blocker_gate_allows_starting_when_the_blocker_is_done() -> Result<()> {
    let (service, _tmp, tx, task) = service_with_recipe().await?;

    let blocker = service
        .create_node(Node::new(
            "issue".to_string(),
            "Finished blocker".to_string(),
            serde_json::json!({ "status": "done" }),
        ))
        .await?;
    let blocked = service
        .create_node(Node::new(
            "issue".to_string(),
            "Now unblocked".to_string(),
            serde_json::json!({ "status": "open" }),
        ))
        .await?;
    service
        .create_relationship(&blocker, "blocks", &blocked, serde_json::json!({}))
        .await?;

    let node = service.get_node(&blocked).await?.expect("exists");
    service
        .update_node(
            &blocked,
            node.version,
            nodespace_core::models::NodeUpdate::default()
                .with_properties(serde_json::json!({ "status": "in_progress" })),
        )
        .await
        .expect("a resolved blocker must not block the start");

    shutdown(tx, task).await
}

/// The sub-issue gate must reject closing a parent with an open child.
#[tokio::test]
async fn sub_issue_gate_rejects_closing_a_parent_with_an_open_child() -> Result<()> {
    let (service, _tmp, tx, task) = service_with_recipe().await?;

    let parent = service
        .create_node(Node::new(
            "issue".to_string(),
            "Parent".to_string(),
            serde_json::json!({ "status": "in_progress" }),
        ))
        .await?;

    service
        .create_node_with_parent(child_params(&parent, "Open child", "open"))
        .await?;

    let node = service.get_node(&parent).await?.expect("parent exists");
    let result = service
        .update_node(
            &parent,
            node.version,
            nodespace_core::models::NodeUpdate::default()
                .with_properties(serde_json::json!({ "status": "done" })),
        )
        .await;

    assert!(
        result.is_err(),
        "closing an issue with an open sub-issue must be rejected"
    );

    shutdown(tx, task).await
}

/// Closing a parent whose children are all done must succeed.
#[tokio::test]
async fn sub_issue_gate_allows_closing_when_every_child_is_done() -> Result<()> {
    let (service, _tmp, tx, task) = service_with_recipe().await?;

    let parent = service
        .create_node(Node::new(
            "issue".to_string(),
            "Parent".to_string(),
            serde_json::json!({ "status": "in_progress" }),
        ))
        .await?;

    service
        .create_node_with_parent(child_params(&parent, "Done child", "done"))
        .await?;

    let node = service.get_node(&parent).await?.expect("parent exists");
    service
        .update_node(
            &parent,
            node.version,
            nodespace_core::models::NodeUpdate::default()
                .with_properties(serde_json::json!({ "status": "done" })),
        )
        .await
        .expect("all children done — the parent must be closeable");

    shutdown(tx, task).await
}

/// Rollover must MOVE a task, not copy it.
///
/// This is the test whose absence let a commit claiming to fix the
/// leaves-it-in-both-cycles bug pass the whole gate without the fix. Adding an
/// edge to the successor is not a move: only forward cardinality is enforced
/// on write, `cycle.tasks` is `many` on that side, and the idempotency check
/// is keyed on `(source, target, name)` — so nothing rejects a second cycle
/// claiming the same task. Counting the edges afterwards is the only way to
/// see it.
///
/// Drives the rule's actions directly rather than waiting for the cron tick:
/// `CronRunner` wakes on a 60-second poll, which no test should sit through,
/// and what is under test is what the actions DO once they run.
#[tokio::test]
async fn rollover_moves_a_task_rather_than_leaving_it_in_both_cycles() -> Result<()> {
    let (service, _tmp, tx, task) = service_with_recipe().await?;

    let ending = service
        .create_node(Node::new(
            "cycle".to_string(),
            "Ending cycle".to_string(),
            serde_json::json!({
                "start_date": "2026-01-01",
                "end_date": chrono::Utc::now().format("%Y-%m-%d").to_string(),
                "duration_days": 14,
            }),
        ))
        .await?;
    let work = service
        .create_node(Node::new(
            "issue".to_string(),
            "Unfinished work".to_string(),
            serde_json::json!({ "status": "in_progress" }),
        ))
        .await?;
    service
        .create_relationship(&ending, "tasks", &work, serde_json::json!({}))
        .await?;

    assert_eq!(
        service
            .get_related_nodes(&ending, "tasks", "out")
            .await?
            .len(),
        1,
        "precondition: the ending cycle holds the task"
    );

    run_rollover(&service, &ending).await?;

    let cycles = service.query_nodes_by_type("cycle", Some("active")).await?;
    let successor = cycles
        .iter()
        .find(|c| c.id != ending)
        .expect("the rule should have created a successor cycle");

    let in_successor = service
        .get_related_nodes(successor.id.as_str(), "tasks", "out")
        .await?;
    assert_eq!(in_successor.len(), 1, "the task must land in the successor");
    assert_eq!(
        in_successor[0].id, work,
        "the successor must hold the original task, not a copy"
    );

    assert_eq!(
        service
            .get_related_nodes(&ending, "tasks", "out")
            .await?
            .len(),
        0,
        "the task must be GONE from the ending cycle — an add without a remove \
         leaves it in both, so every later estimate sum double-counts it"
    );

    shutdown(tx, task).await
}

/// Run the rollover play's single rule against `trigger`, the way the
/// CronRunner would once its cron matched.
async fn run_rollover(service: &Arc<NodeService>, trigger_id: &str) -> Result<()> {
    use nodespace_core::db::events::{DomainEvent, PlaybookExecutionContext};
    use nodespace_core::playbook::types::{parse_rule, parse_rules_from_properties};

    let play_id = "linear-cycle-rollover";
    let play = service
        .get_node(play_id)
        .await?
        .unwrap_or_else(|| panic!("{play_id} should be installed"));
    let trigger = service
        .get_node(trigger_id)
        .await?
        .expect("trigger node exists");

    let defs = parse_rules_from_properties(&play.properties)
        .map_err(|e| anyhow::anyhow!("parsing {play_id}: {e:?}"))?;
    let rule = parse_rule(defs.first().expect("the play declares a rule"))
        .map_err(|e| anyhow::anyhow!("parsing rule: {e:?}"))?;

    let event = DomainEvent::NodeCreated {
        node_type: trigger.node_type.clone(),
        node_id: trigger.id.clone(),
    };
    let ctx = PlaybookExecutionContext {
        originating_event_id: "test-rollover".to_string(),
        depth: 1,
        source_playbook_id: play_id.to_string(),
    };

    match nodespace_core::playbook::actions::execute_actions(
        &rule.actions,
        &trigger,
        &event,
        service,
        ctx,
    )
    .await
    {
        nodespace_core::playbook::actions::ActionResult::Success => Ok(()),
        nodespace_core::playbook::actions::ActionResult::Failed(e) => {
            Err(anyhow::anyhow!("rollover actions failed: {e}"))
        }
    }
}

/// The relationship viewer must count an inherited edge between two subtypes.
///
/// Making `get_inbound_relationships` subtype-aware is what surfaced
/// `task.blocks` as an inbound declaration for `issue` — correct, and what
/// makes the blocker gate work. But the viewer then narrowed each group to
/// nodes whose type equalled the DECLARER's (`task`), so an `issue` on the far
/// end was dropped and the group rendered a confident "0" rather than a
/// visibly missing entry.
///
/// A count that is wrong reads as truth; a group that is absent at least reads
/// as absent. That is why this is worth a test of its own.
#[tokio::test]
async fn the_relationship_viewer_counts_an_inherited_edge_between_subtypes() -> Result<()> {
    let (service, _tmp, tx, task) = service_with_recipe().await?;

    let blocker = service
        .create_node(Node::new(
            "issue".to_string(),
            "The blocker".to_string(),
            serde_json::json!({ "status": "open" }),
        ))
        .await?;
    let blocked = service
        .create_node(Node::new(
            "issue".to_string(),
            "The blocked issue".to_string(),
            serde_json::json!({ "status": "open" }),
        ))
        .await?;
    service
        .create_relationship(&blocker, "blocks", &blocked, serde_json::json!({}))
        .await?;

    let groups = nodespace_core::ops::rel_ops::get_node_relationships(&service, &blocked)
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?;

    let blocks_group = groups
        .groups
        .iter()
        .find(|g| g.relationship_name == "blocks" && g.direction == "in")
        .expect("an inbound `blocks` group should exist for a blocked issue");

    assert_eq!(
        blocks_group.count, 1,
        "the blocking issue must be counted — an exact type match against the \
         declaring schema (`task`) drops every subtype instance and renders 0"
    );
    assert_eq!(blocks_group.related[0].id, blocker);

    shutdown(tx, task).await
}
