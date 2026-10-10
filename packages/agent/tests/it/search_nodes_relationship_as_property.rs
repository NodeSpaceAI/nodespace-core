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

/// Only an equality is read as a relationship to a node. "Tasks not assigned to
/// Anoop" run as "assigned to Anoop" would answer the opposite question, so a
/// negated or non-equality filter is left as written and refused.
#[tokio::test(flavor = "multi_thread")]
async fn a_negated_or_non_equality_filter_is_not_read_as_the_relationship() {
    let (executor, _tmp) = executor_with_assigned_tasks().await;

    for (operator, negate) in [("equals", true), ("contains", false)] {
        let refused = executor
            .execute(
                "search_nodes",
                json!({
                    "node_type": "task",
                    "filters": [{
                        "type": "property", "operator": operator, "negate": negate,
                        "property": "assignee", "value": ANOOP
                    }]
                }),
            )
            .await
            .expect_err("a filter that is not an equality must not be rewritten")
            .to_string();
        assert!(
            refused.contains("is a relationship of 'task'"),
            "{operator}: {refused}"
        );
    }
}

/// A property that is no relationship, written beside an id, is not guessed
/// into one just because the type has a relationship to that kind of node.
#[tokio::test(flavor = "multi_thread")]
async fn an_unrelated_property_beside_an_id_is_not_read_as_a_relationship() {
    let (executor, _tmp) = executor_with_assigned_tasks().await;

    let result = executor
        .execute(
            "search_nodes",
            json!({
                "node_type": "task",
                "filters": [{
                    "type": "property", "operator": "equals",
                    "property": "notes", "value": ANOOP
                }]
            }),
        )
        .await
        .expect("an unknown property runs and matches nothing");

    assert!(result.result.get("filters_rewritten").is_none());
    assert_eq!(result.result["count"], 0);
}

/// A relationship filter whose id sits beside a label for an id is repaired; one
/// whose value sits beside a real property name is a comparison on the far
/// node, and keeps it.
#[tokio::test(flavor = "multi_thread")]
async fn only_an_id_label_makes_the_value_the_node_to_reach() {
    let (executor, _tmp) = executor_with_assigned_tasks().await;

    let labelled = executor
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
    assert_eq!(ids(&labelled.result), [TASK_A, TASK_B]);
    assert!(labelled.result.get("filters_rewritten").is_some());

    // `first_name` is a property of the person reached, so the value is what it
    // is compared with, not a node to reach: nothing is repaired.
    let compared = executor
        .execute(
            "search_nodes",
            json!({
                "node_type": "task",
                "filters": [{
                    "type": "relationship", "operator": "equals",
                    "path": ["assignee"], "property": "first_name", "value": ANOOP
                }]
            }),
        )
        .await;
    if let Ok(compared) = compared {
        assert!(compared.result.get("filters_rewritten").is_none());
    }

    // Not an equality: left for the query to refuse, not repaired.
    let negated = executor
        .execute(
            "search_nodes",
            json!({
                "node_type": "task",
                "filters": [{
                    "type": "relationship", "operator": "equals", "negate": true,
                    "path": ["assignee"], "property": "person-id", "value": ANOOP
                }]
            }),
        )
        .await;
    if let Ok(negated) = negated {
        assert!(negated.result.get("filters_rewritten").is_none());
    }
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

/// Writing a relationship's name as a field value stores a property that
/// changes nothing a reader sees. The call from a real chat, `custom:assignee`
/// on a task, is refused with the call that sets it, and that call works.
#[tokio::test(flavor = "multi_thread")]
async fn a_field_value_naming_a_relationship_is_refused_and_the_edge_is_set_the_named_way() {
    let (executor, _tmp) = executor_with_assigned_tasks().await;

    for key in ["custom:assignee", "assignee", "task__assignee"] {
        let refused = executor
            .execute(
                "update_node",
                json!({ "id": TASK_C, "field_values": { key: "Norbert Weber" } }),
            )
            .await
            .expect_err("a relationship is not a field")
            .to_string();
        assert!(
            refused.contains("is a relationship of 'task'"),
            "{key}: {refused}"
        );
        assert!(
            refused.contains("relationship_type 'tasks'"),
            "{key}: {refused}"
        );
    }

    let created = executor
        .execute(
            "create_relationship",
            json!({ "from_id": NORBERT, "to_id": TASK_C, "relationship_type": "tasks" }),
        )
        .await
        .expect("the named call sets the assignee");
    assert_eq!(created.result["created"], true);

    let found = executor
        .execute(
            "search_nodes",
            json!({
                "node_type": "task",
                "filters": [{ "type": "relationship", "operator": "equals",
                              "path": ["assignee"], "node_id": NORBERT }]
            }),
        )
        .await
        .expect("search_nodes must run");
    assert_eq!(ids(&found.result), [TASK_C]);
}

/// The refusal is on creation as well, and a built-in inbound name (`child_of`)
/// is not one a field value is refused for: it has no declaring end to write
/// from, so the store decides.
#[tokio::test(flavor = "multi_thread")]
async fn creating_a_node_refuses_a_relationship_key_and_a_built_in_inbound_name_is_left_alone() {
    let (executor, _tmp) = executor_with_assigned_tasks().await;

    let refused = executor
        .execute(
            "create_node",
            json!({
                "node_type": "task", "content": "plan",
                "field_values": { "custom:assignee": "Norbert Weber" }
            }),
        )
        .await
        .expect_err("a relationship is not a field")
        .to_string();
    assert!(refused.contains("is a relationship of 'task'"), "{refused}");

    let left = executor
        .execute(
            "update_node",
            json!({ "id": TASK_C, "field_values": { "child_of": "x" } }),
        )
        .await;
    if let Err(e) = left {
        assert!(
            !e.to_string().contains("is a relationship of 'task'"),
            "{e}"
        );
    }
}
