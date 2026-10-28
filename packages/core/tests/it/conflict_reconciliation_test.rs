//! `NodeService::reconcile_conflicts` (ADR-068 §5.4, S4): the reconciliation
//! sweep closes open conflict records whose participants are gone or no
//! longer collide, and leaves genuinely still-colliding records open.

use anyhow::Result;
use nodespace_core::db::SqliteStore;
use nodespace_core::models::conflict::ConflictStatus;
use nodespace_core::models::{Node, NodeUpdate};
use nodespace_core::schema::handle_create_schema;
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
            String::new(),
            json!({ "person": { "first_name": "Alice", "email": "alice@example.com" } }),
        ))
        .await?;
    let bob_id = svc
        .create_node(Node::new(
            "person".to_string(),
            String::new(),
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
            String::new(),
            json!({ "person": { "first_name": "Alice", "email": "alice@example.com" } }),
        ))
        .await?;
    let bob_id = svc
        .create_node(Node::new(
            "person".to_string(),
            String::new(),
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
            String::new(),
            json!({ "person": { "first_name": "Alice", "email": "alice@example.com" } }),
        ))
        .await?;
    let _bob_id = svc
        .create_node(Node::new(
            "person".to_string(),
            String::new(),
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
            String::new(),
            json!({ "person": { "first_name": "Alice", "email": "alice@example.com" } }),
        ))
        .await?;
    let _bob_id = svc
        .create_node(Node::new(
            "person".to_string(),
            String::new(),
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
                String::new(),
                json!({ "person": { "first_name": "Alice", "email": "alice@example.com" } }),
            ))
            .await?;
        let bob_id = svc
            .create_node(Node::new(
                "person".to_string(),
                String::new(),
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

/// Base type declares `email` as `unique`; the subtype extends the base
/// without redeclaring the field, so its value is stored in the ancestor's
/// bucket (`bucket_properties_by_owner`, ADR-078) rather than the subtype's
/// own.
async fn create_reconcile_base_and_subtype(svc: &Arc<NodeService>) -> Result<()> {
    handle_create_schema(
        svc,
        json!({
            "name": "reconcile-ext-base",
            "fields": [{
                "name": "email",
                "type": "text",
                "protection": "user",
                "indexed": false,
                "unique": true
            }]
        }),
    )
    .await
    .map_err(|e| anyhow::anyhow!("base schema: {e}"))?;

    handle_create_schema(
        svc,
        json!({
            "name": "reconcile-ext-sub",
            "extends": "reconcile-ext-base",
            "fields": []
        }),
    )
    .await
    .map_err(|e| anyhow::anyhow!("subtype schema: {e}"))?;
    Ok(())
}

/// The bug the fix above closes for an inherited field:
/// `unique_field_collision_still_holds` used to re-derive each participant's
/// current value from `node.properties[node_type]`, which is empty for a
/// field inherited from an ancestor schema (the value lives in the
/// ancestor's bucket instead). That made the sweep find `None` for every
/// participant of a `UniqueFieldCollision` record on an inherited field,
/// fall through to `Ok(false)`, and auto-close a record naming a collision
/// that was still fully valid and unchanged.
#[tokio::test]
async fn sweep_leaves_a_still_colliding_inherited_field_record_open() -> Result<()> {
    let (svc, _tmp) = service().await?;
    let svc = Arc::new(svc);
    create_reconcile_base_and_subtype(&svc).await?;

    let alice_id = svc
        .create_node(Node::new(
            "reconcile-ext-sub".to_string(),
            "Alice".to_string(),
            json!({ "reconcile-ext-sub": { "email": "alice@example.com" } }),
        ))
        .await?;
    let _bob_id = svc
        .create_node(Node::new(
            "reconcile-ext-sub".to_string(),
            "Bob".to_string(),
            json!({ "reconcile-ext-sub": { "email": "alice@example.com" } }),
        ))
        .await?;

    let records = svc.conflicts_for_node(&alice_id).await?;
    assert_eq!(
        records.len(),
        1,
        "the colliding inherited-field email must have journaled a conflict"
    );

    let closed = svc.reconcile_conflicts().await?;
    assert_eq!(
        closed, 0,
        "a real, unresolved collision on an inherited unique field must not \
         be auto-closed by the sweep"
    );

    let after = svc.conflicts_for_node(&alice_id).await?;
    assert_eq!(after[0].status, ConflictStatus::Open);

    Ok(())
}

/// The no-longer-colliding counterpart: once the inherited field's value
/// genuinely diverges, the sweep must still correctly close the record (not
/// just correctly leave a real one open).
#[tokio::test]
async fn sweep_closes_an_inherited_field_collision_that_no_longer_holds() -> Result<()> {
    let (svc, _tmp) = service().await?;
    let svc = Arc::new(svc);
    create_reconcile_base_and_subtype(&svc).await?;

    let alice_id = svc
        .create_node(Node::new(
            "reconcile-ext-sub".to_string(),
            "Alice".to_string(),
            json!({ "reconcile-ext-sub": { "email": "alice@example.com" } }),
        ))
        .await?;
    let bob_id = svc
        .create_node(Node::new(
            "reconcile-ext-sub".to_string(),
            "Bob".to_string(),
            json!({ "reconcile-ext-sub": { "email": "alice@example.com" } }),
        ))
        .await?;

    let records = svc.conflicts_for_node(&alice_id).await?;
    assert_eq!(records.len(), 1);

    let bob = svc.get_node(&bob_id).await?.unwrap();
    svc.update_node(
        &bob_id,
        bob.version,
        NodeUpdate::new()
            .with_properties(json!({ "reconcile-ext-sub": { "email": "bob@example.com" } })),
    )
    .await?;

    let closed = svc.reconcile_conflicts().await?;
    assert_eq!(closed, 1);

    let after = svc.conflicts_for_node(&alice_id).await?;
    assert_eq!(after[0].status, ConflictStatus::Resolved);
    let resolution = after[0].resolution.as_ref().unwrap();
    assert_eq!(resolution["reason"], "no_longer_conflicting");

    Ok(())
}

// ---------------------------------------------------------------------------
// Collection-name collisions
// ---------------------------------------------------------------------------

/// Two collection ids, the first sorting before the second. A record's ids
/// are stored sorted, so every scenario below runs with each one created
/// first: a predicate that leans on id or row order fails one of them.
const LOW_COLLECTION_ID: &str = "1b6f1c2e-3d4a-4b5c-8d6e-7f8091a2b3c4";
const HIGH_COLLECTION_ID: &str = "e4d3c2b1-a091-4f8e-9d7c-6b5a4f3e2d1c";

/// Creates two active collections named "Projects", `first` before `second`,
/// and checks that their collision is journaled as one open record.
async fn colliding_collections(svc: &NodeService, first: &str, second: &str) -> Result<()> {
    colliding_collections_named(svc, first, second, "Projects").await
}

/// [`colliding_collections`] with the shared name given.
async fn colliding_collections_named(
    svc: &NodeService,
    first: &str,
    second: &str,
    name: &str,
) -> Result<()> {
    for id in [first, second] {
        svc.create_node(Node::new_with_id(
            id.to_string(),
            "collection".to_string(),
            name.to_string(),
            json!({}),
        ))
        .await?;
    }
    let records = svc.conflicts_for_node(first).await?;
    assert_eq!(records.len(), 1, "the name collision must be journaled");
    assert_eq!(records[0].status, ConflictStatus::Open);
    Ok(())
}

#[tokio::test]
async fn sweep_leaves_a_live_collection_name_collision_open_in_either_id_order() -> Result<()> {
    for (first, second) in [
        (LOW_COLLECTION_ID, HIGH_COLLECTION_ID),
        (HIGH_COLLECTION_ID, LOW_COLLECTION_ID),
    ] {
        let (svc, _tmp) = service().await?;
        colliding_collections(&svc, first, second).await?;

        let closed = svc.reconcile_conflicts().await?;
        assert_eq!(
            closed, 0,
            "two active collections still share the name ({first} created first)"
        );
        let after = svc.conflicts_for_node(first).await?;
        assert_eq!(after[0].status, ConflictStatus::Open);
    }
    Ok(())
}

/// A name with a non-ASCII capital is folded the same way when the collision
/// is journaled and when the sweep re-checks it, so a live collision stays
/// open.
#[tokio::test]
async fn sweep_leaves_a_live_collision_on_a_non_ascii_capital_name_open() -> Result<()> {
    for (first, second) in [
        (LOW_COLLECTION_ID, HIGH_COLLECTION_ID),
        (HIGH_COLLECTION_ID, LOW_COLLECTION_ID),
    ] {
        let (svc, _tmp) = service().await?;
        colliding_collections_named(&svc, first, second, "Équipe").await?;

        let closed = svc.reconcile_conflicts().await?;
        assert_eq!(closed, 0, "the collision on \"Équipe\" is still live");
        let after = svc.conflicts_for_node(first).await?;
        assert_eq!(after[0].status, ConflictStatus::Open);
    }
    Ok(())
}

/// Archiving or renaming either participant outside the Conflicts view ends
/// the collision, whichever one it is and whichever was created first.
#[tokio::test]
async fn sweep_closes_a_collection_name_collision_once_either_participant_leaves_it() -> Result<()>
{
    for (first, second) in [
        (LOW_COLLECTION_ID, HIGH_COLLECTION_ID),
        (HIGH_COLLECTION_ID, LOW_COLLECTION_ID),
    ] {
        for leaving in [LOW_COLLECTION_ID, HIGH_COLLECTION_ID] {
            for update in [
                NodeUpdate::new().with_lifecycle_status("archived".to_string()),
                NodeUpdate::new().with_content("Archive".to_string()),
            ] {
                let (svc, _tmp) = service().await?;
                colliding_collections(&svc, first, second).await?;
                let node = svc.get_node(leaving).await?.unwrap();
                svc.update_node(leaving, node.version, update.clone())
                    .await?;

                let closed = svc.reconcile_conflicts().await?;
                assert_eq!(
                    closed, 1,
                    "{leaving} left the collision ({first} created first, {update:?})"
                );
                let after = svc.conflicts_for_node(first).await?;
                assert_eq!(after[0].status, ConflictStatus::Resolved);
                assert_eq!(
                    after[0].resolution.as_ref().unwrap()["reason"],
                    "no_longer_conflicting"
                );
            }
        }
    }
    Ok(())
}
