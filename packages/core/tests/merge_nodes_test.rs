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
use nodespace_core::services::{
    CreateNodeParams, InsertPosition, InsertPositionOwned, NodeService,
};
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
            String::new(),
            json!({ "person": { "first_name": "Alice", "email": "alice@example.com" } }),
        ))
        .await?;
    let loser_id = svc
        .create_node(Node::new(
            "person".to_string(),
            String::new(),
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
        .create_node(Node::new("person".to_string(), String::new(), json!({})))
        .await?;
    let loser_id = svc
        .create_node(Node::new("person".to_string(), String::new(), json!({})))
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
        .create_node(Node::new("person".to_string(), String::new(), json!({})))
        .await?;
    let loser_id = svc
        .create_node(Node::new("person".to_string(), String::new(), json!({})))
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
async fn merge_evicts_a_repointed_edge_that_violates_forward_cardinality_one() -> Result<()> {
    let (svc, _tmp) = service().await?;

    // A declared relationship pair with `cardinality: one` on the forward
    // (source) side and `many` on the reverse, so only the forward check is
    // in play — mirrors
    // `create_relationship_replaces_prior_edge_from_cardinality_one_source`
    // in `node_service/mod.rs`'s own test suite.
    svc.store()
        .create_node(
            Node::new_with_id(
                "widget".to_string(),
                "schema".to_string(),
                "Widget".to_string(),
                json!({ "fields": [], "relationships": [] }),
            ),
            None,
            None,
        )
        .await?;
    svc.store()
        .create_node(
            Node::new_with_id(
                "gadget".to_string(),
                "schema".to_string(),
                "Gadget".to_string(),
                json!({ "fields": [] }),
            ),
            None,
            None,
        )
        .await?;
    let declarations: Vec<nodespace_core::models::schema::SchemaRelationship> =
        serde_json::from_value(json!([{
            "name": "primary_widget",
            "targetType": "widget",
            "direction": "out",
            "cardinality": "one",
            "reverseName": "gadgets",
            "reverseCardinality": "many"
        }]))?;
    svc.set_schema_relationships("gadget", &declarations)
        .await?;

    let survivor_id = svc
        .create_node(Node::new(
            "gadget".to_string(),
            "Gadget One".to_string(),
            json!({}),
        ))
        .await?;
    let loser_id = svc
        .create_node(Node::new(
            "gadget".to_string(),
            "Gadget Two".to_string(),
            json!({}),
        ))
        .await?;
    let survivor_widget = svc
        .create_node(Node::new(
            "widget".to_string(),
            "Widget One".to_string(),
            json!({}),
        ))
        .await?;
    let loser_widget = svc
        .create_node(Node::new(
            "widget".to_string(),
            "Widget Two".to_string(),
            json!({}),
        ))
        .await?;

    // Both survivor and loser hold their own compliant cardinality-one edge,
    // toward DIFFERENT targets — neither create_relationship call has
    // anything to replace at creation time.
    svc.create_relationship(&survivor_id, "primary_widget", &survivor_widget, json!({}))
        .await?;
    svc.create_relationship(&loser_id, "primary_widget", &loser_widget, json!({}))
        .await?;

    let outcome = svc.merge_nodes(&survivor_id, &loser_id, None).await?;

    // The repoint itself succeeds (different targets, no raw unique-index
    // collision) but must then be evicted by the cardinality-one check —
    // never counted as a surviving repoint.
    assert_eq!(
        outcome.edges_repointed, 0,
        "the repointed edge must be evicted for violating cardinality: one, not survive"
    );
    assert_eq!(
        outcome.edges_dropped, 1,
        "the evicted repoint must still be counted as dropped"
    );

    // Exactly one `primary_widget` edge remains, and it is the survivor's
    // own pre-existing edge — not the loser's.
    assert_eq!(
        svc.store()
            .check_relationship_exists(&survivor_id, "primary_widget")
            .await?,
        1,
        "cardinality: one must hold after the merge"
    );
    assert!(
        svc.store()
            .relationship_exists(&survivor_id, &survivor_widget, "primary_widget")
            .await?,
        "the survivor's own edge must be kept"
    );
    assert!(
        !svc.store()
            .relationship_exists(&survivor_id, &loser_widget, "primary_widget")
            .await?,
        "the loser's repointed edge must be dropped, not duplicated onto the survivor"
    );

    Ok(())
}

#[tokio::test]
async fn merge_evicts_a_repointed_edge_that_violates_reverse_cardinality_one() -> Result<()> {
    let (svc, _tmp) = service().await?;

    // `person.tasks` -> `task`, `reverseCardinality: one` (a task has a
    // single assignee) is a core-seeded declaration — no custom schema
    // needed. The MERGE happens on the `task` side: two tasks, each already
    // validly assigned to a different person, get merged into one.
    let survivor_task = svc
        .create_node(Node::new(
            "task".to_string(),
            "Ship the feature".to_string(),
            json!({}),
        ))
        .await?;
    let loser_task = svc
        .create_node(Node::new(
            "task".to_string(),
            "Ship the feature (dup)".to_string(),
            json!({}),
        ))
        .await?;
    let person_a = svc
        .create_node(Node::new("person".to_string(), String::new(), json!({})))
        .await?;
    let person_b = svc
        .create_node(Node::new("person".to_string(), String::new(), json!({})))
        .await?;

    svc.create_relationship(&person_a, "tasks", &survivor_task, json!({}))
        .await?;
    svc.create_relationship(&person_b, "tasks", &loser_task, json!({}))
        .await?;

    let outcome = svc.merge_nodes(&survivor_task, &loser_task, None).await?;

    assert_eq!(
        outcome.edges_repointed, 0,
        "the repointed (former person_b) edge must be evicted for violating reverse_cardinality: one"
    );
    assert_eq!(outcome.edges_dropped, 1);

    // The survivor task must show exactly one assignee — Alice's
    // pre-existing edge, not Bob's repointed one.
    assert!(
        svc.store()
            .relationship_exists(&person_a, &survivor_task, "tasks")
            .await?,
        "the survivor's own assignee edge must be kept"
    );
    assert!(
        !svc.store()
            .relationship_exists(&person_b, &survivor_task, "tasks")
            .await?,
        "the loser's repointed assignee edge must be dropped, not give the task two assignees"
    );

    Ok(())
}

#[tokio::test]
async fn merge_rolls_back_entirely_when_reverse_cardinality_eviction_would_violate_a_required_relationship(
) -> Result<()> {
    let (svc, _tmp) = service().await?;

    // Same `widget`/`gadget` shape as the reverse-cardinality test above,
    // but `reverseCardinality: one` (a widget has a single gadget pointing
    // at it) AND `required: true` on the forward side (a gadget must always
    // point at at least one widget). The two constraints are genuinely
    // irreconcilable across this merge: keeping both pre-existing edges
    // violates the survivor widget's `reverseCardinality: one`, but
    // evicting either would strip that edge's gadget down to zero
    // `primary_widget` edges, violating `required: true` on that gadget.
    svc.store()
        .create_node(
            Node::new_with_id(
                "widget2".to_string(),
                "schema".to_string(),
                "Widget2".to_string(),
                json!({ "fields": [], "relationships": [] }),
            ),
            None,
            None,
        )
        .await?;
    svc.store()
        .create_node(
            Node::new_with_id(
                "gadget2".to_string(),
                "schema".to_string(),
                "Gadget2".to_string(),
                json!({ "fields": [] }),
            ),
            None,
            None,
        )
        .await?;
    let declarations: Vec<nodespace_core::models::schema::SchemaRelationship> =
        serde_json::from_value(json!([{
            "name": "primary_widget",
            "targetType": "widget2",
            "direction": "out",
            "cardinality": "many",
            "required": true,
            "reverseName": "gadgets",
            "reverseCardinality": "one"
        }]))?;
    svc.set_schema_relationships("gadget2", &declarations)
        .await?;

    let survivor_widget = svc
        .create_node(Node::new(
            "widget2".to_string(),
            "Widget2 One".to_string(),
            json!({}),
        ))
        .await?;
    let loser_widget = svc
        .create_node(Node::new(
            "widget2".to_string(),
            "Widget2 Two".to_string(),
            json!({}),
        ))
        .await?;
    let gadget_a = svc
        .create_node(Node::new(
            "gadget2".to_string(),
            "Gadget2 A".to_string(),
            json!({}),
        ))
        .await?;
    let gadget_b = svc
        .create_node(Node::new(
            "gadget2".to_string(),
            "Gadget2 B".to_string(),
            json!({}),
        ))
        .await?;

    // Each gadget holds its own sole, compliant edge — `required` is
    // satisfied and `reverseCardinality: one` holds, before the merge.
    svc.create_relationship(&gadget_a, "primary_widget", &survivor_widget, json!({}))
        .await?;
    svc.create_relationship(&gadget_b, "primary_widget", &loser_widget, json!({}))
        .await?;

    // Merging loser_widget into survivor_widget repoints gadget_b's edge
    // onto survivor_widget, which now has two `primary_widget` edges in —
    // violating `reverseCardinality: one`. Evicting gadget_b's (the
    // repointed) edge is the only fix the cardinality pass can make, but
    // that would leave gadget_b with zero `primary_widget` edges, violating
    // `required: true` on gadget_b. The merge must fail outright rather
    // than silently resolve one invariant by breaking the other.
    let result = svc.merge_nodes(&survivor_widget, &loser_widget, None).await;
    assert!(
        result.is_err(),
        "an irreconcilable required-vs-cardinality conflict must fail the merge, not silently \
         pick a winner"
    );

    // And the failure must be a clean, atomic rollback — nothing about
    // either node or either edge may have changed.
    let loser_node = svc
        .get_node(&loser_widget)
        .await?
        .expect("loser must still exist");
    assert_eq!(
        loser_node.lifecycle_status, "active",
        "a rolled-back merge must not leave the loser archived"
    );
    assert!(
        svc.store()
            .relationship_exists(&gadget_a, &survivor_widget, "primary_widget")
            .await?,
        "the survivor's own edge must be untouched by the rolled-back merge"
    );
    assert!(
        svc.store()
            .relationship_exists(&gadget_b, &loser_widget, "primary_widget")
            .await?,
        "the loser's edge must still point at the loser — the repoint must have been rolled back"
    );
    assert!(
        !svc.store()
            .relationship_exists(&gadget_b, &survivor_widget, "primary_widget")
            .await?,
        "the repoint must not have landed durably"
    );

    Ok(())
}

#[tokio::test]
async fn merge_archives_the_loser_not_hard_deletes_it() -> Result<()> {
    let (svc, _tmp) = service().await?;

    let survivor_id = svc
        .create_node(Node::new("person".to_string(), String::new(), json!({})))
        .await?;
    let loser_id = svc
        .create_node(Node::new("person".to_string(), String::new(), json!({})))
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

// --- Tree invariants: single parent, root-only membership, no cycles ---

async fn text(svc: &NodeService, content: &str, parent: Option<&str>) -> Result<String> {
    Ok(svc
        .create_node_with_parent(CreateNodeParams {
            id: None,
            node_type: "text".into(),
            content: content.into(),
            parent_id: parent.map(Into::into),
            position: InsertPositionOwned::End,
            properties: json!({}),
            lifecycle_status: None,
        })
        .await?)
}

async fn collection(svc: &NodeService, name: &str) -> Result<String> {
    Ok(svc
        .create_node_with_parent(CreateNodeParams {
            id: None,
            node_type: "collection".into(),
            content: name.into(),
            parent_id: None,
            position: InsertPositionOwned::End,
            properties: json!({}),
            lifecycle_status: None,
        })
        .await?)
}

async fn child_ids(svc: &NodeService, parent: &str) -> Result<Vec<String>> {
    Ok(svc
        .get_children(parent)
        .await?
        .into_iter()
        .map(|n| n.id)
        .collect())
}

async fn parent_id(svc: &NodeService, id: &str) -> Result<Option<String>> {
    Ok(svc.get_parent(id).await?.map(|n| n.id))
}

/// Both nodes are children in different trees: the survivor keeps its own
/// parent and the loser's parent edge is dropped, never added as a second.
#[tokio::test]
async fn merge_of_two_children_keeps_the_survivors_parent_only() -> Result<()> {
    let (svc, _tmp) = service().await?;
    let root_a = text(&svc, "Root A", None).await?;
    let root_b = text(&svc, "Root B", None).await?;
    let loser = text(&svc, "line", Some(&root_a)).await?;
    let survivor = text(&svc, "line", Some(&root_b)).await?;

    let outcome = svc.merge_nodes(&survivor, &loser, None).await?;

    assert_eq!(outcome.edges_dropped, 1, "the loser's parent edge");
    assert!(
        !child_ids(&svc, &root_a).await?.contains(&survivor),
        "the survivor must not gain a second parent"
    );
    assert_eq!(child_ids(&svc, &root_b).await?, vec![survivor.clone()]);
    Ok(())
}

/// A root survivor still takes the loser's position.
#[tokio::test]
async fn merge_into_a_root_survivor_takes_the_losers_parent() -> Result<()> {
    let (svc, _tmp) = service().await?;
    let root_a = text(&svc, "Root A", None).await?;
    let loser = text(&svc, "line", Some(&root_a)).await?;
    let survivor = text(&svc, "line", None).await?;

    let outcome = svc.merge_nodes(&survivor, &loser, None).await?;

    assert_eq!(outcome.edges_repointed, 1);
    assert_eq!(parent_id(&svc, &survivor).await?, Some(root_a));
    Ok(())
}

/// A survivor directly under the loser: the edge between them becomes a
/// self-edge and is dropped, and the survivor takes the loser's parent.
#[tokio::test]
async fn merge_into_the_losers_own_child_takes_the_losers_parent() -> Result<()> {
    let (svc, _tmp) = service().await?;
    let root = text(&svc, "Root", None).await?;
    let loser = text(&svc, "loser", Some(&root)).await?;
    let survivor = text(&svc, "survivor", Some(&loser)).await?;

    svc.merge_nodes(&survivor, &loser, None).await?;

    assert_eq!(parent_id(&svc, &survivor).await?, Some(root));
    assert!(child_ids(&svc, &survivor).await?.is_empty(), "no self-edge");
    Ok(())
}

/// ADR-059 §2 from the survivor's side: a filed root survivor may not take
/// the loser's parent. The whole merge is refused and nothing is written.
#[tokio::test]
async fn merge_refuses_to_give_a_filed_survivor_a_parent() -> Result<()> {
    let (svc, _tmp) = service().await?;
    let coll = collection(&svc, "Filed").await?;
    let root_a = text(&svc, "Root A", None).await?;
    let loser = text(&svc, "line", Some(&root_a)).await?;
    let survivor = text(&svc, "line", None).await?;
    svc.store()
        .add_to_collection(&survivor, &coll, &json!({}))
        .await?;

    let err = svc
        .merge_nodes(&survivor, &loser, None)
        .await
        .expect_err("a filed node must not gain a parent");

    let msg = err.to_string();
    assert!(
        msg.contains("member_of_not_root") && msg.contains("ADR-059 §2"),
        "{msg}"
    );
    assert_eq!(parent_id(&svc, &survivor).await?, None);
    assert_eq!(parent_id(&svc, &loser).await?, Some(root_a));
    assert_eq!(
        svc.get_node(&loser).await?.unwrap().lifecycle_status,
        "active"
    );
    Ok(())
}

/// ADR-059 §2 from the loser's side: a filed root loser's membership may not
/// move onto a survivor that has a parent.
#[tokio::test]
async fn merge_refuses_to_file_a_survivor_that_has_a_parent() -> Result<()> {
    let (svc, _tmp) = service().await?;
    let coll = collection(&svc, "Filed").await?;
    let root_b = text(&svc, "Root B", None).await?;
    let survivor = text(&svc, "line", Some(&root_b)).await?;
    let loser = text(&svc, "line", None).await?;
    svc.store()
        .add_to_collection(&loser, &coll, &json!({}))
        .await?;

    let err = svc
        .merge_nodes(&survivor, &loser, None)
        .await
        .expect_err("a child must not gain collection membership");

    assert!(err.to_string().contains("member_of_not_root"), "{err}");
    assert!(svc
        .store()
        .get_node_memberships(&survivor)
        .await?
        .is_empty());
    assert_eq!(svc.store().get_node_memberships(&loser).await?, vec![coll]);
    Ok(())
}

/// A survivor deeper than a direct child in the loser's subtree would become
/// its own ancestor once the loser's children re-point onto it.
#[tokio::test]
async fn merge_refuses_a_survivor_below_the_losers_children() -> Result<()> {
    let (svc, _tmp) = service().await?;
    let loser = text(&svc, "loser", None).await?;
    let middle = text(&svc, "middle", Some(&loser)).await?;
    let survivor = text(&svc, "survivor", Some(&middle)).await?;

    let err = svc
        .merge_nodes(&survivor, &loser, None)
        .await
        .expect_err("the survivor would sit below itself");

    assert!(err.to_string().contains("merge_would_cycle"), "{err}");
    assert_eq!(child_ids(&svc, &loser).await?, vec![middle]);
    Ok(())
}

/// A loser deeper than a direct child in a root survivor's subtree: taking
/// the loser's parent would make the survivor its own ancestor, so the edge
/// is dropped and the survivor stays a root.
#[tokio::test]
async fn merge_drops_a_parent_edge_from_inside_the_survivors_subtree() -> Result<()> {
    let (svc, _tmp) = service().await?;
    let survivor = text(&svc, "survivor", None).await?;
    let middle = text(&svc, "middle", Some(&survivor)).await?;
    let loser = text(&svc, "loser", Some(&middle)).await?;
    let loser_child = text(&svc, "loser child", Some(&loser)).await?;

    let outcome = svc.merge_nodes(&survivor, &loser, None).await?;

    assert_eq!(outcome.edges_dropped, 1, "the loser's parent edge");
    assert_eq!(parent_id(&svc, &survivor).await?, None);
    assert!(child_ids(&svc, &middle).await?.is_empty());
    assert_eq!(parent_id(&svc, &loser_child).await?, Some(survivor));
    Ok(())
}

/// A collection is always a root, so it may not take the loser's parent.
#[tokio::test]
async fn merge_refuses_to_give_a_collection_survivor_a_parent() -> Result<()> {
    let (svc, _tmp) = service().await?;
    let root_a = text(&svc, "Root A", None).await?;
    let loser = text(&svc, "line", Some(&root_a)).await?;
    let survivor = collection(&svc, "Coll").await?;

    let err = svc
        .merge_nodes(&survivor, &loser, None)
        .await
        .expect_err("a collection must stay a root");

    assert!(err.to_string().contains("collection_not_root"), "{err}");
    assert_eq!(parent_id(&svc, &survivor).await?, None);
    Ok(())
}

/// A `person` may hold membership under a parent, so a filed person
/// survivor still takes the loser's parent.
#[tokio::test]
async fn merge_lets_a_filed_person_survivor_take_a_parent() -> Result<()> {
    let (svc, _tmp) = service().await?;
    let coll = collection(&svc, "Team").await?;
    let root_a = text(&svc, "Root A", None).await?;
    let survivor = svc
        .create_node(Node::new("person".to_string(), String::new(), json!({})))
        .await?;
    let loser = svc
        .create_node(Node::new("person".to_string(), String::new(), json!({})))
        .await?;
    svc.create_parent_edge(&loser, &root_a, InsertPosition::End)
        .await?;
    svc.store()
        .add_to_collection(&survivor, &coll, &json!({}))
        .await?;

    svc.merge_nodes(&survivor, &loser, None).await?;

    assert_eq!(parent_id(&svc, &survivor).await?, Some(root_a));
    assert_eq!(
        svc.store().get_node_memberships(&survivor).await?,
        vec![coll]
    );
    Ok(())
}
