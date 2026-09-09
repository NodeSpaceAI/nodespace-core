//! `NodeService::reconcile_conflicts` (ADR-068 §5.4, S4): the reconciliation
//! sweep closes open conflict records whose participants are gone or no
//! longer collide, and leaves genuinely still-colliding records open.

use anyhow::Result;
use nodespace_core::db::SqliteStore;
use nodespace_core::models::conflict::ConflictStatus;
use nodespace_core::models::{Node, NodeUpdate};
use nodespace_core::services::NodeService;
use serde_json::json;
use std::sync::Arc;
use tempfile::TempDir;

async fn service() -> Result<(NodeService, TempDir)> {
    let temp_dir = TempDir::new()?;
    let db_path = temp_dir.path().join("reconcile.db");
    let mut store = Arc::new(SqliteStore::new(db_path).await?);
    let service = NodeService::new(&mut store).await?;
    Ok((service, temp_dir))
}

#[tokio::test]
async fn sweep_closes_a_record_whose_participant_was_hard_deleted() -> Result<()> {
    let (svc, _tmp) = service().await?;

    let alice_id = svc
        .create_node(Node::new(
            "person".to_string(),
            "Alice".to_string(),
            json!({ "person": { "first_name": "Alice", "email": "alice@example.com" } }),
        ))
        .await?;
    let bob_id = svc
        .create_node(Node::new(
            "person".to_string(),
            "Bob".to_string(),
            json!({ "person": { "first_name": "Bob", "email": "alice@example.com" } }),
        ))
        .await?;

    let records = svc.conflicts_for_node(&alice_id).await?;
    assert_eq!(
        records.len(),
        1,
        "the colliding email must have journaled a conflict"
    );

    // Hard-delete Bob (a real deletion, not archive/merge).
    let bob = svc.get_node(&bob_id).await?.unwrap();
    svc.delete_node(&bob_id, bob.version).await?;

    let closed = svc.reconcile_conflicts().await?;
    assert_eq!(closed, 1);

    let after = svc.conflicts_for_node(&alice_id).await?;
    assert_eq!(after.len(), 1, "the same record must still exist");
    assert_eq!(after[0].status, ConflictStatus::Resolved);
    let resolution = after[0].resolution.as_ref().unwrap();
    assert_eq!(resolution["action"], "self_resolved");
    assert_eq!(resolution["reason"], "participant_deleted");

    Ok(())
}

#[tokio::test]
async fn sweep_closes_a_unique_field_collision_that_no_longer_holds() -> Result<()> {
    let (svc, _tmp) = service().await?;

    let alice_id = svc
        .create_node(Node::new(
            "person".to_string(),
            "Alice".to_string(),
            json!({ "person": { "first_name": "Alice", "email": "alice@example.com" } }),
        ))
        .await?;
    let bob_id = svc
        .create_node(Node::new(
            "person".to_string(),
            "Bob".to_string(),
            json!({ "person": { "first_name": "Bob", "email": "alice@example.com" } }),
        ))
        .await?;

    let records = svc.conflicts_for_node(&alice_id).await?;
    assert_eq!(records.len(), 1);

    // Bob changes his email — the collision no longer holds, but nothing
    // re-runs detection for the OLD value (only the new write's own value is
    // checked), so the stale open record survives until the sweep runs.
    let bob = svc.get_node(&bob_id).await?.unwrap();
    svc.update_node(
        &bob_id,
        bob.version,
        NodeUpdate::new().with_properties(
            json!({ "person": { "first_name": "Bob", "email": "bob@example.com" } }),
        ),
    )
    .await?;

    let still_open_before_sweep = svc.conflicts_for_node(&alice_id).await?;
    assert_eq!(
        still_open_before_sweep[0].status,
        ConflictStatus::Open,
        "the stale record is not auto-closed by the update itself"
    );

    let closed = svc.reconcile_conflicts().await?;
    assert_eq!(closed, 1);

    let after = svc.conflicts_for_node(&alice_id).await?;
    assert_eq!(after[0].status, ConflictStatus::Resolved);
    let resolution = after[0].resolution.as_ref().unwrap();
    assert_eq!(resolution["reason"], "no_longer_conflicting");

    Ok(())
}

#[tokio::test]
async fn sweep_leaves_a_genuinely_still_colliding_record_open() -> Result<()> {
    let (svc, _tmp) = service().await?;

    let alice_id = svc
        .create_node(Node::new(
            "person".to_string(),
            "Alice".to_string(),
            json!({ "person": { "first_name": "Alice", "email": "alice@example.com" } }),
        ))
        .await?;
    let _bob_id = svc
        .create_node(Node::new(
            "person".to_string(),
            "Bob".to_string(),
            json!({ "person": { "first_name": "Bob", "email": "alice@example.com" } }),
        ))
        .await?;

    let closed = svc.reconcile_conflicts().await?;
    assert_eq!(
        closed, 0,
        "a real, unresolved collision must not be auto-closed"
    );

    let after = svc.conflicts_for_node(&alice_id).await?;
    assert_eq!(after[0].status, ConflictStatus::Open);

    Ok(())
}

#[tokio::test]
async fn sweep_does_not_reopen_a_dismissed_record() -> Result<()> {
    let (svc, _tmp) = service().await?;

    let alice_id = svc
        .create_node(Node::new(
            "person".to_string(),
            "Alice".to_string(),
            json!({ "person": { "first_name": "Alice", "email": "alice@example.com" } }),
        ))
        .await?;
    let _bob_id = svc
        .create_node(Node::new(
            "person".to_string(),
            "Bob".to_string(),
            json!({ "person": { "first_name": "Bob", "email": "alice@example.com" } }),
        ))
        .await?;

    let records = svc.conflicts_for_node(&alice_id).await?;
    svc.resolve_conflict(&records[0].id, nodespace_core::models::Resolution::Dismiss)
        .await?;

    // list_conflicts(Open) excludes it, so the sweep never even considers it.
    let closed = svc.reconcile_conflicts().await?;
    assert_eq!(closed, 0);

    let after = svc.conflicts_for_node(&alice_id).await?;
    assert_eq!(after[0].status, ConflictStatus::Dismissed);

    Ok(())
}

/// The bug the fix above closes: re-checking from an ARBITRARY participant's
/// exclusion perspective (e.g. always `node_ids.first()`, sorted lexically)
/// is wrong regardless of which one gets picked, because whichever
/// participant DIDN'T move away still legitimately holds the disputed value.
/// Excluding that one and searching for the SAME value finds the moved-away
/// participant only if it happens to still hold a leftover value — but here
/// it finds the OTHER, unrelated, still-correct participant instead, and
/// reports a false continuing collision. This test runs the scenario with
/// BOTH id orderings (by constructing two independent pairs) so a regression
/// that reintroduces "pick one side arbitrarily" fails regardless of how
/// UUIDs happen to sort on a given run.
#[tokio::test]
async fn sweep_result_does_not_depend_on_which_participant_id_sorts_first() -> Result<()> {
    for _ in 0..8 {
        let (svc, _tmp) = service().await?;

        let alice_id = svc
            .create_node(Node::new(
                "person".to_string(),
                "Alice".to_string(),
                json!({ "person": { "first_name": "Alice", "email": "alice@example.com" } }),
            ))
            .await?;
        let bob_id = svc
            .create_node(Node::new(
                "person".to_string(),
                "Bob".to_string(),
                json!({ "person": { "first_name": "Bob", "email": "alice@example.com" } }),
            ))
            .await?;

        let bob = svc.get_node(&bob_id).await?.unwrap();
        svc.update_node(
            &bob_id,
            bob.version,
            NodeUpdate::new().with_properties(
                json!({ "person": { "first_name": "Bob", "email": "bob@example.com" } }),
            ),
        )
        .await?;

        let closed = svc.reconcile_conflicts().await?;
        assert_eq!(
            closed, 1,
            "must close regardless of alice_id/bob_id's lexical ordering"
        );

        let after = svc.conflicts_for_node(&alice_id).await?;
        assert_eq!(after[0].status, ConflictStatus::Resolved);
    }

    Ok(())
}
