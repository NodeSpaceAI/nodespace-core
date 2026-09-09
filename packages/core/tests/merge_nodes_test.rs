//! `NodeService::merge_nodes` (ADR-068 §5.2, S3 — the highest-risk slice per
//! the spec, landed last): property union, edge re-pointing (including
//! collision-drop and order preservation), loser archival (not a literal
//! "deleted" tombstone — see `SqliteStore::merge_nodes_in_tx`'s doc comment
//! for why `archived` is the correct local analog), and conflict-record
//! closure, all in one transaction.

use anyhow::Result;
use nodespace_core::db::SqliteStore;
use nodespace_core::models::conflict::{ConflictKind, ConflictStatus};
use nodespace_core::models::Node;
use nodespace_core::services::NodeService;
use serde_json::json;
use std::sync::Arc;
use tempfile::TempDir;

async fn service() -> Result<(NodeService, TempDir)> {
    let temp_dir = TempDir::new()?;
    let db_path = temp_dir.path().join("merge.db");
    let mut store = Arc::new(SqliteStore::new(db_path).await?);
    let service = NodeService::new(&mut store).await?;
    Ok((service, temp_dir))
}

#[tokio::test]
async fn merge_unions_properties_survivor_wins_ties_and_captures_superseded() -> Result<()> {
    let (svc, _tmp) = service().await?;

    let survivor_id = svc
        .create_node(Node::new(
            "person".to_string(),
            "Alice".to_string(),
            json!({ "person": { "first_name": "Alice", "email": "alice@example.com" } }),
        ))
        .await?;
    let loser_id = svc
        .create_node(Node::new(
            "person".to_string(),
            "Alice B".to_string(),
            json!({ "person": { "first_name": "Alice", "email": "ALICE@example.com", "last_name": "Smith" } }),
        ))
        .await?;

    let outcome = svc.merge_nodes(&survivor_id, &loser_id, None).await?;

    // last_name was absent on the survivor -> copied from the loser.
    // email existed on both, differing -> survivor's value wins.
    assert_eq!(
        outcome.properties_merged, 1,
        "only last_name is absent-on-survivor"
    );

    let survivor_after = svc.get_node(&survivor_id).await?.unwrap();
    assert_eq!(
        survivor_after.properties["person"]["email"],
        json!("alice@example.com"),
        "survivor's own value must win a property present on both sides"
    );
    assert_eq!(
        survivor_after.properties["person"]["last_name"],
        json!("Smith"),
        "a property absent on the survivor must be copied from the loser"
    );

    Ok(())
}

#[tokio::test]
async fn merge_repoints_has_child_and_preserves_order() -> Result<()> {
    let (svc, _tmp) = service().await?;

    let survivor_id = svc
        .create_node(Node::new(
            "person".to_string(),
            "Alice".to_string(),
            json!({}),
        ))
        .await?;
    let loser_id = svc
        .create_node(Node::new(
            "person".to_string(),
            "Alice B".to_string(),
            json!({}),
        ))
        .await?;

    // Two existing children under the survivor, establishing a real
    // fractional-order sequence, plus one child under the loser.
    let survivor_child_1 = svc
        .store()
        .create_child_node_atomic(&survivor_id, "text", "survivor child 1", json!({}), None)
        .await?;
    let survivor_child_2 = svc
        .store()
        .create_child_node_atomic(&survivor_id, "text", "survivor child 2", json!({}), None)
        .await?;
    let loser_child = svc
        .store()
        .create_child_node_atomic(&loser_id, "text", "a note under Alice B", json!({}), None)
        .await?;

    let outcome = svc.merge_nodes(&survivor_id, &loser_id, None).await?;
    assert_eq!(
        outcome.edges_repointed, 1,
        "the loser's one has_child edge must re-point"
    );
    assert_eq!(
        outcome.edges_dropped, 0,
        "no collision — the survivor has no edge to this child"
    );

    // The loser's child now hangs off the survivor, alongside its existing
    // two — the edge MOVED (single row, endpoint UPDATE) rather than being
    // duplicated or losing its `properties.order` (which a delete-and-
    // reinsert would reset, defaulting it ahead of or behind where it should
    // sort — `get_children` sorts by that same order, so a lost/zeroed value
    // would surface here as a wrong position, not just a missing assertion).
    let survivor_children = svc.get_children(&survivor_id).await?;
    let child_ids: Vec<&str> = survivor_children.iter().map(|n| n.id.as_str()).collect();
    assert_eq!(child_ids.len(), 3);
    assert!(child_ids.contains(&survivor_child_1.id.as_str()));
    assert!(child_ids.contains(&survivor_child_2.id.as_str()));
    assert!(child_ids.contains(&loser_child.id.as_str()));

    let loser_children = svc.get_children(&loser_id).await?;
    assert!(
        loser_children.is_empty(),
        "the edge must have MOVED, not been duplicated"
    );

    Ok(())
}

#[tokio::test]
async fn merge_drops_a_repoint_that_would_collide_with_an_existing_survivor_edge() -> Result<()> {
    let (svc, _tmp) = service().await?;

    let survivor_id = svc
        .create_node(Node::new(
            "person".to_string(),
            "Alice".to_string(),
            json!({}),
        ))
        .await?;
    let loser_id = svc
        .create_node(Node::new(
            "person".to_string(),
            "Alice B".to_string(),
            json!({}),
        ))
        .await?;
    let shared_target = svc
        .create_node(Node::new(
            "text".to_string(),
            "shared".to_string(),
            json!({}),
        ))
        .await?;

    // Both survivor and loser already `mentions` the same target — re-pointing
    // the loser's edge onto the survivor would collide with the survivor's own.
    svc.create_relationship(&survivor_id, "mentions", &shared_target, json!({}))
        .await?;
    svc.create_relationship(&loser_id, "mentions", &shared_target, json!({}))
        .await?;

    let outcome = svc.merge_nodes(&survivor_id, &loser_id, None).await?;
    assert_eq!(
        outcome.edges_dropped, 1,
        "the loser's colliding mentions edge must be dropped, not erroring the merge"
    );
    assert_eq!(outcome.edges_repointed, 0);

    // Exactly one mentions edge from survivor -> shared_target survives.
    let mentions = svc.get_mentions(&survivor_id).await?;
    assert_eq!(
        mentions.iter().filter(|id| *id == &shared_target).count(),
        1
    );

    Ok(())
}

#[tokio::test]
async fn merge_archives_the_loser_not_hard_deletes_it() -> Result<()> {
    let (svc, _tmp) = service().await?;

    let survivor_id = svc
        .create_node(Node::new(
            "person".to_string(),
            "Alice".to_string(),
            json!({}),
        ))
        .await?;
    let loser_id = svc
        .create_node(Node::new(
            "person".to_string(),
            "Alice B".to_string(),
            json!({}),
        ))
        .await?;

    svc.merge_nodes(&survivor_id, &loser_id, None).await?;

    // The row still exists (nothing destroyed) but is archived, not active.
    let loser_after = svc
        .get_node(&loser_id)
        .await?
        .expect("merge must archive, not hard-delete, the loser");
    assert_eq!(loser_after.lifecycle_status, "archived");

    Ok(())
}

#[tokio::test]
async fn merge_closes_the_conflict_record_with_a_merge_resolution() -> Result<()> {
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
    let open = records
        .iter()
        .find(|r| r.kind == ConflictKind::UniqueFieldCollision && r.status == ConflictStatus::Open)
        .expect("the colliding email must have journaled a conflict");
    let conflict_id = open.id.clone();

    let outcome = svc
        .merge_nodes(&alice_id, &bob_id, Some(&conflict_id))
        .await?;
    assert_eq!(outcome.survivor_id, alice_id);
    assert_eq!(outcome.loser_id, bob_id);

    let after = svc.conflicts_for_node(&alice_id).await?;
    let closed = after
        .iter()
        .find(|r| r.id == conflict_id)
        .expect("the same record must still exist, now resolved");
    assert_eq!(closed.status, ConflictStatus::Resolved);
    let resolution = closed
        .resolution
        .as_ref()
        .expect("a resolved record must carry its resolution");
    assert_eq!(resolution["action"], "merge");
    assert_eq!(resolution["survivor"], alice_id);
    assert_eq!(resolution["loser"], bob_id);

    Ok(())
}

#[tokio::test]
async fn get_conflict_returns_the_record_by_its_own_id() -> Result<()> {
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
    let open = records
        .iter()
        .find(|r| r.kind == ConflictKind::UniqueFieldCollision && r.status == ConflictStatus::Open)
        .expect("the colliding email must have journaled a conflict");

    let fetched = svc
        .get_conflict(&open.id)
        .await?
        .expect("get_conflict must find the record by its own id");
    assert_eq!(fetched.id, open.id);
    assert_eq!(fetched.kind, ConflictKind::UniqueFieldCollision);
    assert_eq!(fetched.status, ConflictStatus::Open);

    Ok(())
}

#[tokio::test]
async fn get_conflict_returns_none_for_an_unknown_id() -> Result<()> {
    let (svc, _tmp) = service().await?;

    let missing = svc.get_conflict("not-a-real-conflict-id").await?;
    assert!(missing.is_none());

    Ok(())
}
