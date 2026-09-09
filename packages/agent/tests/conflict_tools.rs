//! Integration tests for the conflict-journal agent tools (ADR-068):
//! `list_conflicts`, `get_conflict`, `dismiss_conflict`,
//! `adopt_existing_conflict`, `merge_conflict`. Drives the real production
//! `GraphToolExecutor::execute(...)` surface against a real `SqliteStore`,
//! so assertions cover the same call path the local agent actually takes.

use std::sync::Arc;

use nodespace_agent::local_agent::tools::GraphToolExecutor;
use nodespace_agent::AgentToolExecutor;
use nodespace_core::db::SqliteStore;
use nodespace_core::models::Node;
use nodespace_core::services::NodeService;
use serde_json::json;
use tempfile::TempDir;
use tokio::sync::RwLock;

async fn make_executor() -> (GraphToolExecutor, Arc<NodeService>, TempDir) {
    let tmp = TempDir::new().unwrap();
    let db_path = tmp.path().join("test.db");
    let mut store: Arc<SqliteStore> = Arc::new(SqliteStore::new(db_path).await.unwrap());
    let ns = Arc::new(NodeService::new(&mut store).await.unwrap());
    let executor = GraphToolExecutor {
        node_service: Some(ns.clone()),
        embedding_service: Arc::new(RwLock::new(None)),
        inference_engine: None,
    };
    (executor, ns, tmp)
}

/// `person`'s `email` field is a system-seeded `unique_case_insensitive`
/// field (`NodeService::new` seeds it), so creating two `person` nodes with
/// the same folded email trips the create-path `detect_unique_field_collisions`
/// hook and journals an open `UniqueFieldCollision` record naming both — no
/// explicit `create_schema` call needed. Mirrors
/// `packages/core/tests/person_duplicate_convergence_test.rs`'s fixture.
///
/// `email_local_part` must be distinct per call within a test — the field is
/// unique across every active `person` node in the store, so reusing a value
/// would collide a second pair with the first pair's nodes too.
async fn seed_colliding_people(ns: &NodeService, email_local_part: &str) -> (String, String) {
    let email = format!("{email_local_part}@example.com");
    let alice = ns
        .create_node(Node::new(
            "person".to_string(),
            "Alice".to_string(),
            json!({ "person": { "email": email } }),
        ))
        .await
        .expect("seed alice");
    let bob = ns
        .create_node(Node::new(
            "person".to_string(),
            "Bob".to_string(),
            json!({ "person": { "email": email.to_uppercase() } }),
        ))
        .await
        .expect("seed bob (colliding email)");
    (alice, bob)
}

async fn open_conflict_for(ns: &NodeService, node_id: &str) -> serde_json::Value {
    let records = ns.conflicts_for_node(node_id).await.unwrap();
    let record = records
        .iter()
        .find(|r| {
            r.kind == nodespace_core::models::conflict::ConflictKind::UniqueFieldCollision
                && r.status == nodespace_core::models::conflict::ConflictStatus::Open
        })
        .expect("the colliding email must have journaled a conflict");
    serde_json::to_value(record).unwrap()
}

#[tokio::test]
async fn list_conflicts_finds_a_journaled_collision() {
    let (executor, ns, _tmp) = make_executor().await;
    let (alice, bob) = seed_colliding_people(&ns, "list-all").await;

    let result = executor
        .execute("list_conflicts", json!({}))
        .await
        .expect("list_conflicts must succeed");

    assert!(!result.is_error, "list_conflicts returned an error result");
    let count = result.result["count"].as_u64().unwrap();
    assert_eq!(count, 1, "expected exactly the one journaled collision");
    let node_ids = result.result["conflicts"][0]["nodeIds"]
        .as_array()
        .expect("nodeIds must be an array");
    let ids: Vec<&str> = node_ids.iter().map(|v| v.as_str().unwrap()).collect();
    assert!(ids.contains(&alice.as_str()));
    assert!(ids.contains(&bob.as_str()));
}

#[tokio::test]
async fn list_conflicts_by_node_filters_to_that_participant() {
    let (executor, ns, _tmp) = make_executor().await;
    let (alice, _bob) = seed_colliding_people(&ns, "list-by-node").await;

    // An unrelated node with no conflicts must not show up.
    let unrelated = ns
        .create_node(Node::new(
            "text".to_string(),
            "Unrelated".to_string(),
            json!({}),
        ))
        .await
        .unwrap();

    let result = executor
        .execute("list_conflicts", json!({ "node": alice }))
        .await
        .expect("list_conflicts by node must succeed");
    assert_eq!(result.result["count"], 1);

    let result = executor
        .execute("list_conflicts", json!({ "node": unrelated }))
        .await
        .expect("list_conflicts by node must succeed");
    assert_eq!(result.result["count"], 0);
}

#[tokio::test]
async fn get_conflict_returns_the_full_record() {
    let (executor, ns, _tmp) = make_executor().await;
    let (alice, _bob) = seed_colliding_people(&ns, "get-conflict").await;
    let record = open_conflict_for(&ns, &alice).await;
    let conflict_id = record["id"].as_str().unwrap().to_string();

    let result = executor
        .execute("get_conflict", json!({ "conflict_id": conflict_id }))
        .await
        .expect("get_conflict must succeed");

    assert!(!result.is_error);
    assert_eq!(result.result["id"], conflict_id);
    assert_eq!(result.result["kind"], "uniqueFieldCollision");
}

#[tokio::test]
async fn get_conflict_reports_an_error_result_for_an_unknown_id() {
    let (executor, _ns, _tmp) = make_executor().await;

    let result = executor
        .execute("get_conflict", json!({ "conflict_id": "does-not-exist" }))
        .await
        .expect("get_conflict call must not itself error");

    assert!(
        result.is_error,
        "an unknown conflict id must be an error result"
    );
}

#[tokio::test]
async fn dismiss_conflict_closes_it_as_dismissed_and_survives_redetection() {
    let (executor, ns, _tmp) = make_executor().await;
    let (alice, bob) = seed_colliding_people(&ns, "dismiss").await;
    let record = open_conflict_for(&ns, &alice).await;
    let conflict_id = record["id"].as_str().unwrap().to_string();

    let result = executor
        .execute(
            "dismiss_conflict",
            json!({ "conflict_id": conflict_id.clone() }),
        )
        .await
        .expect("dismiss_conflict must succeed");
    assert!(!result.is_error);
    assert_eq!(result.result["status"], "dismissed");

    // Re-detection (an edit that still runs the collision check) must not
    // reopen the dismissed record — the whole point of a durable resolution
    // over the old `_possible_duplicate` flag.
    nodespace_core::ops::node_ops::update_node(
        &ns,
        nodespace_core::ops::node_ops::UpdateNodeInput {
            node_id: bob.clone(),
            version: None,
            node_type: None,
            content: Some("Bob (touched)".to_string()),
            properties: None,
            add_to_collections: Vec::new(),
            add_to_collection_ids: Vec::new(),
            remove_from_collection_ids: Vec::new(),
            lifecycle_status: None,
        },
    )
    .await
    .expect("touching bob's content must succeed");

    let after = ns.conflicts_for_node(&alice).await.unwrap();
    let still = after.iter().find(|r| r.id == conflict_id).unwrap();
    assert_eq!(
        still.status,
        nodespace_core::models::conflict::ConflictStatus::Dismissed,
        "a dismissed conflict must not be silently reopened"
    );
}

#[tokio::test]
async fn adopt_existing_conflict_resolves_without_touching_either_node() {
    let (executor, ns, _tmp) = make_executor().await;
    let (alice, bob) = seed_colliding_people(&ns, "adopt").await;
    let record = open_conflict_for(&ns, &alice).await;
    let conflict_id = record["id"].as_str().unwrap().to_string();

    let result = executor
        .execute(
            "adopt_existing_conflict",
            json!({ "conflict_id": conflict_id, "keep": alice }),
        )
        .await
        .expect("adopt_existing_conflict must succeed");

    assert!(!result.is_error);
    assert_eq!(result.result["status"], "resolved");
    assert_eq!(result.result["resolution"]["action"], "adopt_existing");
    assert_eq!(result.result["resolution"]["adopted"], alice);

    // Neither node was deleted or archived.
    let alice_after = ns.get_node(&alice).await.unwrap().unwrap();
    let bob_after = ns.get_node(&bob).await.unwrap().unwrap();
    assert_eq!(alice_after.lifecycle_status, "active");
    assert_eq!(bob_after.lifecycle_status, "active");
}

#[tokio::test]
async fn merge_conflict_archives_the_loser_and_closes_the_record() {
    let (executor, ns, _tmp) = make_executor().await;
    let (alice, bob) = seed_colliding_people(&ns, "merge").await;
    let record = open_conflict_for(&ns, &alice).await;
    let conflict_id = record["id"].as_str().unwrap().to_string();

    let result = executor
        .execute(
            "merge_conflict",
            json!({
                "survivor_id": alice,
                "loser_id": bob,
                "conflict_id": conflict_id.clone(),
            }),
        )
        .await
        .expect("merge_conflict must succeed");

    assert!(!result.is_error);
    assert_eq!(result.result["survivor_id"], format!("nodespace://{alice}"));
    assert_eq!(result.result["loser_id"], format!("nodespace://{bob}"));

    let loser_after = ns.get_node(&bob).await.unwrap().unwrap();
    assert_eq!(loser_after.lifecycle_status, "archived");

    let after = ns.conflicts_for_node(&alice).await.unwrap();
    let closed = after.iter().find(|r| r.id == conflict_id).unwrap();
    assert_eq!(
        closed.status,
        nodespace_core::models::conflict::ConflictStatus::Resolved
    );
    assert_eq!(
        closed.resolution.as_ref().unwrap()["action"],
        json!("merge")
    );
}

/// `merge_conflict` is registered as removing user data (it archives the
/// loser), matching `delete_node`'s classification — this pins that the
/// wire-level tool actually IS in the registry's destructive set, as a
/// cross-check against the unit-level pin in `local_agent::tools`'s own
/// tests.
#[tokio::test]
async fn merge_conflict_is_registered_and_classified_destructive() {
    use nodespace_agent::local_agent::tools::{removes_user_data_tool, Tool};

    assert_eq!(Tool::from_name("merge_conflict"), Some(Tool::MergeConflict));
    assert!(removes_user_data_tool("merge_conflict"));
    assert!(!removes_user_data_tool("dismiss_conflict"));
    assert!(!removes_user_data_tool("adopt_existing_conflict"));
    assert!(!removes_user_data_tool("list_conflicts"));
    assert!(!removes_user_data_tool("get_conflict"));
}
