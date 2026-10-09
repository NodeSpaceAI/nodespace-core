//! A `search_nodes` property filter that names a relationship.
//!
//! A task's `assignee` is the other end of a person's `tasks`. Filtered as a
//! property it matches nothing, and an empty result reads as "nobody has any".
//! A model reaches for it anyway, with the person's id inside the value. The
//! tool runs the call as the relationship filter it meant and says so; with no
//! node to reach, the query refuses it and names the filter to use.

use std::sync::Arc;

use nodespace_agent::local_agent::tools::GraphToolExecutor;
use nodespace_agent::AgentToolExecutor;
use nodespace_core::db::SqliteStore;
use nodespace_core::models::Node;
use nodespace_core::services::NodeService;
use serde_json::{json, Value};
use tempfile::TempDir;
use tokio::sync::RwLock;

const ANOOP: &str = "5b1c7a30-2e4d-4f68-9a13-7c0d8e6f1a01";
const NORBERT: &str = "5b1c7a30-2e4d-4f68-9a13-7c0d8e6f1a02";
const TASK_A: &str = "5b1c7a30-2e4d-4f68-9a13-7c0d8e6f1a03";
const TASK_B: &str = "5b1c7a30-2e4d-4f68-9a13-7c0d8e6f1a04";
const TASK_C: &str = "5b1c7a30-2e4d-4f68-9a13-7c0d8e6f1a05";

async fn executor_with_assigned_tasks() -> (GraphToolExecutor, TempDir) {
    let tmp = TempDir::new().unwrap();
    let mut store: Arc<SqliteStore> =
        Arc::new(SqliteStore::new(tmp.path().join("test.db")).await.unwrap());
    let ns = Arc::new(NodeService::new(&mut store).await.unwrap());

    for (id, first, last) in [(ANOOP, "Anoop", "Nair"), (NORBERT, "Norbert", "Weber")] {
        ns.create_node(Node::new_with_id(
            id.to_string(),
            "person".to_string(),
            String::new(),
            json!({ "first_name": first, "last_name": last }),
        ))
        .await
        .unwrap();
    }
    for id in [TASK_A, TASK_B, TASK_C] {
        ns.create_node(Node::new_with_id(
            id.to_string(),
            "task".to_string(),
            format!("task {id}"),
            json!({}),
        ))
        .await
        .unwrap();
    }
    for task in [TASK_A, TASK_B] {
        ns.create_relationship(ANOOP, "tasks", task, json!({}))
            .await
            .unwrap();
    }

    let executor = GraphToolExecutor {
        node_service: Some(ns),
        embedding_service: Arc::new(RwLock::new(None)),
        inference_engine: None,
        playbook_lifecycle: None,
    };
    (executor, tmp)
}

fn ids(result: &Value) -> Vec<String> {
    let mut ids: Vec<String> = result["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|n| {
            n["id"]
                .as_str()
                .unwrap()
                .trim_start_matches("nodespace://")
                .to_string()
        })
        .collect();
    ids.sort();
    ids
}

#[tokio::test(flavor = "multi_thread")]
async fn a_property_filter_naming_the_assignee_runs_as_the_relationship_filter() {
    let (executor, _tmp) = executor_with_assigned_tasks().await;

    // The value is as a model writes it: a name with the id inside.
    let result = executor
        .execute(
            "search_nodes",
            json!({
                "node_type": "task",
                "filters": [{
                    "type": "property", "operator": "equals",
                    "property": "assignee",
                    "value": format!("Anoop Nair's id={ANOOP}")
                }]
            }),
        )
        .await
        .expect("search_nodes must run");

    assert_eq!(ids(&result.result), [TASK_A, TASK_B]);
    let rewrites = result.result["filters_rewritten"].as_array().unwrap();
    assert_eq!(rewrites.len(), 1, "{rewrites:?}");
    let note = rewrites[0].as_str().unwrap();
    assert!(
        note.contains("'assignee' is a relationship of 'task'"),
        "{note}"
    );
    assert!(note.contains(ANOOP), "{note}");

    // The same answer for the person with none, rather than a false empty.
    let none = executor
        .execute(
            "search_nodes",
            json!({
                "node_type": "task",
                "filters": [{
                    "type": "property", "operator": "equals",
                    "property": "assignee", "value": NORBERT
                }]
            }),
        )
        .await
        .expect("search_nodes must run");
    assert_eq!(none.result["count"], 0);
}

/// A name the model made up for the relationship (`assigned_to`) is read as
/// the one relationship of the type that reaches that kind of node and whose
/// name begins the same way; a relationship filter that has the id outside
/// `node_id` gets it moved there.
#[tokio::test(flavor = "multi_thread")]
async fn an_invented_name_and_a_misplaced_id_resolve_to_the_relationship_meant() {
    let (executor, _tmp) = executor_with_assigned_tasks().await;

    // `task` reaches a person through `assignee` and `creator`; `assigned_to`
    // begins as only one of them does.
    let invented = executor
        .execute(
            "search_nodes",
            json!({
                "node_type": "task",
                "filters": [{
                    "type": "property", "operator": "equals",
                    "property": "assigned_to", "value": ANOOP
                }]
            }),
        )
        .await
        .expect("search_nodes must run");
    assert_eq!(ids(&invented.result), [TASK_A, TASK_B]);
    let note = invented.result["filters_rewritten"][0].as_str().unwrap();
    assert!(
        note.contains("read as its relationship 'assignee'"),
        "{note}"
    );

    let misplaced = executor
        .execute(
            "search_nodes",
            json!({
                "node_type": "task",
                "filters": [{
                    "type": "relationship", "operator": "equals",
                    "path": ["assignee"], "property": "person-id", "value": ANOOP
                }]
            }),
        )
        .await
        .expect("search_nodes must run");
    assert_eq!(ids(&misplaced.result), [TASK_A, TASK_B]);

    // A name that begins like neither relationship is not guessed at, and the
    // empty result names the relationships a filter can follow.
    let unreadable = executor
        .execute(
            "search_nodes",
            json!({
                "node_type": "task",
                "filters": [{
                    "type": "property", "operator": "equals",
                    "property": "owner", "value": ANOOP
                }]
            }),
        )
        .await
        .expect("search_nodes must run");
    assert_eq!(unreadable.result["count"], 0);
    assert!(unreadable.result.get("filters_rewritten").is_none());
    let relationships = unreadable.result["filterable_relationships"]
        .as_array()
        .expect("an empty typed search lists the relationships");
    assert!(
        relationships
            .iter()
            .any(|r| r["name"] == "assignee" && r["to"] == "person"),
        "{relationships:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_relationship_filter_and_a_real_property_filter_are_left_as_written() {
    let (executor, _tmp) = executor_with_assigned_tasks().await;

    let by_relationship = executor
        .execute(
            "search_nodes",
            json!({
                "node_type": "task",
                "filters": [{
                    "type": "relationship", "operator": "equals",
                    "path": ["assignee"], "node_id": ANOOP
                }]
            }),
        )
        .await
        .expect("search_nodes must run");
    assert_eq!(ids(&by_relationship.result), [TASK_A, TASK_B]);
    assert!(by_relationship.result.get("filters_rewritten").is_none());

    let by_status = executor
        .execute(
            "search_nodes",
            json!({
                "node_type": "task",
                "filters": [{
                    "type": "property", "operator": "equals",
                    "property": "status", "value": "open"
                }]
            }),
        )
        .await
        .expect("search_nodes must run");
    assert!(by_status.result.get("filters_rewritten").is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_relationship_named_as_a_property_without_a_node_is_refused_with_the_filter_to_use() {
    let (executor, _tmp) = executor_with_assigned_tasks().await;

    let refused = executor
        .execute(
            "search_nodes",
            json!({
                "node_type": "task",
                "filters": [{
                    "type": "property", "operator": "equals",
                    "property": "assignee", "value": "Anoop"
                }]
            }),
        )
        .await
        .expect_err("a relationship has no value to compare")
        .to_string();

    assert!(refused.contains("is a relationship of 'task'"), "{refused}");
    assert!(refused.contains("\"path\":[\"assignee\"]"), "{refused}");
}
