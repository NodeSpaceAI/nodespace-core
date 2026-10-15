//! `create_question_task`: a question put down for a person so that a task
//! waits for their answer, in one call.
//!
//! The task, its assignee link, the `blocks` edge onto the stopped task and
//! the project link are one write. A bad end refuses before anything is
//! created, so a model that mixes up ids leaves nothing behind.

use std::sync::Arc;

use nodespace_agent::local_agent::tools::GraphToolExecutor;
use nodespace_agent::{AgentToolExecutor, ToolError};
use nodespace_core::db::SqliteStore;
use nodespace_core::models::Node;
use nodespace_core::services::NodeService;
use serde_json::{json, Value};
use tempfile::TempDir;
use tokio::sync::RwLock;

const ANOOP: &str = "6c2d8b41-3f5e-4a79-8b24-8d1e9f7a2b01";
const MARA: &str = "6c2d8b41-3f5e-4a79-8b24-8d1e9f7a2b02";
const STOPPED: &str = "6c2d8b41-3f5e-4a79-8b24-8d1e9f7a2b03";
const PROJECT: &str = "6c2d8b41-3f5e-4a79-8b24-8d1e9f7a2b04";
const LOOSE: &str = "6c2d8b41-3f5e-4a79-8b24-8d1e9f7a2b05";

const QUESTION: &str = "Does the migration guide cover the v1 export format or only v2?";

/// A graph with two people, a stopped task Mara owns inside a project, and a
/// task that sits in no project.
async fn executor() -> (GraphToolExecutor, Arc<NodeService>, TempDir) {
    let tmp = TempDir::new().unwrap();
    let mut store: Arc<SqliteStore> =
        Arc::new(SqliteStore::new(tmp.path().join("test.db")).await.unwrap());
    let ns = Arc::new(NodeService::new(&mut store).await.unwrap());

    for (id, first, last) in [(ANOOP, "Anoop", "Nair"), (MARA, "Mara", "Quint")] {
        ns.create_node(Node::new_with_id(
            id.to_string(),
            "person".to_string(),
            String::new(),
            json!({ "first_name": first, "last_name": last }),
        ))
        .await
        .unwrap();
    }
    ns.create_node(Node::new_with_id(
        PROJECT.to_string(),
        "project".to_string(),
        "Migration".to_string(),
        json!({}),
    ))
    .await
    .unwrap();
    for (id, title) in [
        (STOPPED, "Write the migration guide"),
        (LOOSE, "Tidy the wiki"),
    ] {
        ns.create_node(Node::new_with_id(
            id.to_string(),
            "task".to_string(),
            title.to_string(),
            json!({ "status": "in_progress" }),
        ))
        .await
        .unwrap();
        ns.create_relationship(MARA, "tasks", id, json!({}))
            .await
            .unwrap();
    }
    ns.create_relationship(PROJECT, "tasks", STOPPED, json!({}))
        .await
        .unwrap();

    let executor = GraphToolExecutor {
        node_service: Some(ns.clone()),
        embedding_service: Arc::new(RwLock::new(None)),
        inference_engine: None,
        playbook_lifecycle: None,
    };
    (executor, ns, tmp)
}

/// The ids at the far end of `relationship` from `id`, as bare ids.
async fn related(
    executor: &GraphToolExecutor,
    id: &str,
    relationship: &str,
    dir: &str,
) -> Vec<String> {
    let result = executor
        .execute(
            "get_related_nodes",
            json!({ "id": id, "relationship_type": relationship, "direction": dir }),
        )
        .await
        .expect("get_related_nodes must run");
    let mut ids: Vec<String> = result.result["nodes"]
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

fn bare(result: &Value, key: &str) -> String {
    result[key]
        .as_str()
        .unwrap()
        .trim_start_matches("nodespace://")
        .to_string()
}

async fn task_count(executor: &GraphToolExecutor) -> usize {
    let result = executor
        .execute("search_nodes", json!({ "node_type": "task" }))
        .await
        .expect("search_nodes must run");
    result.result["nodes"].as_array().unwrap().len()
}

#[tokio::test(flavor = "multi_thread")]
async fn one_call_makes_the_task_assigns_it_blocks_the_stopped_task_and_joins_its_project() {
    let (executor, ns, _tmp) = executor().await;
    let stopped_before = ns.get_node(STOPPED).await.unwrap().unwrap();

    let result = executor
        .execute(
            "create_question_task",
            json!({ "question": QUESTION, "owner_id": ANOOP, "stopped_task_id": STOPPED }),
        )
        .await
        .expect("create_question_task must run");
    assert!(!result.is_error, "{:?}", result.result);

    let question = bare(&result.result, "id");
    assert_eq!(bare(&result.result, "assigned_to"), ANOOP);
    assert_eq!(bare(&result.result, "blocks"), STOPPED);
    assert_eq!(bare(&result.result, "project"), PROJECT);

    // Assigned to the person who owes the answer.
    assert!(related(&executor, ANOOP, "tasks", "out")
        .await
        .contains(&question));
    // Blocking the stopped task, not blocked by it.
    assert_eq!(
        related(&executor, &question, "blocks", "out").await,
        [STOPPED]
    );
    // In the stopped task's project.
    assert!(related(&executor, PROJECT, "tasks", "out")
        .await
        .contains(&question));

    // The stopped task keeps its owner and its status.
    assert!(related(&executor, MARA, "tasks", "out")
        .await
        .contains(&STOPPED.to_string()));
    let stopped = ns.get_node(STOPPED).await.unwrap().unwrap();
    assert_eq!(stopped.properties, stopped_before.properties);
    assert_eq!(stopped.version, stopped_before.version);

    let node = ns.get_node(&question).await.unwrap().unwrap();
    assert_eq!(node.node_type, "task");
    assert_eq!(node.content, QUESTION);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_stopped_task_with_no_project_is_blocked_all_the_same() {
    let (executor, _ns, _tmp) = executor().await;

    let result = executor
        .execute(
            "create_question_task",
            json!({ "question": QUESTION, "owner_id": ANOOP, "stopped_task_id": LOOSE }),
        )
        .await
        .expect("create_question_task must run");

    let question = bare(&result.result, "id");
    assert!(result.result["project"].is_null(), "{:?}", result.result);
    assert_eq!(
        related(&executor, &question, "blocks", "out").await,
        [LOOSE]
    );
    assert!(related(&executor, ANOOP, "tasks", "out")
        .await
        .contains(&question));
}

#[tokio::test(flavor = "multi_thread")]
async fn an_owner_that_does_not_exist_refuses_with_nothing_created() {
    let (executor, _ns, _tmp) = executor().await;
    let before = task_count(&executor).await;

    let error = executor
        .execute(
            "create_question_task",
            json!({
                "question": QUESTION,
                "owner_id": "6c2d8b41-3f5e-4a79-8b24-ffffffffffff",
                "stopped_task_id": STOPPED
            }),
        )
        .await
        .expect_err("a missing owner must be refused");

    assert!(
        matches!(&error, ToolError::UnresolvedId { tool, .. } if tool == "create_question_task"),
        "{error:?}"
    );
    assert_eq!(task_count(&executor).await, before);
}

#[tokio::test(flavor = "multi_thread")]
async fn ids_given_the_wrong_way_round_name_the_fix_and_create_nothing() {
    let (executor, _ns, _tmp) = executor().await;
    let before = task_count(&executor).await;

    // The stopped task where the person belongs, and the person where the
    // task belongs: the mix-up a model makes when it copies ids by position.
    let error = executor
        .execute(
            "create_question_task",
            json!({ "question": QUESTION, "owner_id": STOPPED, "stopped_task_id": ANOOP }),
        )
        .await
        .expect_err("swapped ids must be refused");

    let ToolError::InvalidArguments { reason, .. } = &error else {
        panic!("expected InvalidArguments, got {error:?}");
    };
    assert!(reason.contains("'owner' must be a person"), "{reason}");
    assert!(reason.contains("Nothing was created"), "{reason}");
    assert_eq!(task_count(&executor).await, before);
}

#[tokio::test(flavor = "multi_thread")]
async fn an_empty_question_is_refused() {
    let (executor, _ns, _tmp) = executor().await;
    let before = task_count(&executor).await;

    let error = executor
        .execute(
            "create_question_task",
            json!({ "question": "   ", "owner_id": ANOOP, "stopped_task_id": STOPPED }),
        )
        .await
        .expect_err("an empty question must be refused");

    assert!(
        matches!(error, ToolError::InvalidArguments { .. }),
        "{error:?}"
    );
    assert_eq!(task_count(&executor).await, before);
}

#[tokio::test(flavor = "multi_thread")]
async fn the_task_and_the_person_may_be_named_by_title() {
    let (executor, _ns, _tmp) = executor().await;

    let result = executor
        .execute(
            "create_question_task",
            json!({
                "question": QUESTION,
                "owner": "anoop nair",
                "stopped_task": "Write the migration guide"
            }),
        )
        .await
        .expect("a title that names one node must resolve");

    let question = bare(&result.result, "id");
    assert_eq!(bare(&result.result, "assigned_to"), ANOOP);
    assert_eq!(bare(&result.result, "blocks"), STOPPED);
    assert_eq!(
        related(&executor, &question, "blocks", "out").await,
        [STOPPED]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_title_nothing_has_is_a_missing_node() {
    let (executor, _ns, _tmp) = executor().await;
    let before = task_count(&executor).await;

    let error = executor
        .execute(
            "create_question_task",
            json!({
                "question": QUESTION,
                "owner": ANOOP,
                "stopped_task": "Write the changelog"
            }),
        )
        .await
        .expect_err("an unknown title must be refused");

    assert!(
        matches!(&error, ToolError::UnresolvedId { id, .. } if id == "Write the changelog"),
        "{error:?}"
    );
    assert_eq!(task_count(&executor).await, before);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_title_two_tasks_share_is_refused_with_their_ids() {
    let (executor, ns, _tmp) = executor().await;
    const TWIN: &str = "6c2d8b41-3f5e-4a79-8b24-8d1e9f7a2b06";
    ns.create_node(Node::new_with_id(
        TWIN.to_string(),
        "task".to_string(),
        "Write the migration guide".to_string(),
        json!({}),
    ))
    .await
    .unwrap();
    let before = task_count(&executor).await;

    let error = executor
        .execute(
            "create_question_task",
            json!({
                "question": QUESTION,
                "owner": ANOOP,
                "stopped_task": "Write the migration guide"
            }),
        )
        .await
        .expect_err("a shared title must be refused");

    let ToolError::InvalidArguments { reason, .. } = &error else {
        panic!("expected InvalidArguments, got {error:?}");
    };
    assert!(
        reason.contains(STOPPED) && reason.contains(TWIN),
        "{reason}"
    );
    assert_eq!(task_count(&executor).await, before);
}
