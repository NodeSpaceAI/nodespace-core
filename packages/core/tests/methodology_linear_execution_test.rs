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
