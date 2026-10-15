//! `create_task`: a task that is ready the moment it exists, in one call.
//!
//! The task, a text node for each paragraph of its brief, an unticked
//! checkbox for each checklist item and the link from its project are one
//! write. A bad project refuses before anything is created, so a model that
//! mixes up an id leaves nothing behind.

use std::sync::Arc;

use nodespace_agent::local_agent::tools::GraphToolExecutor;
use nodespace_agent::{AgentToolExecutor, ToolError};
use nodespace_core::db::SqliteStore;
use nodespace_core::models::Node;
use nodespace_core::services::NodeService;
use serde_json::{json, Value};
use tempfile::TempDir;
use tokio::sync::RwLock;

const PROJECT: &str = "7d3e9c52-4a6f-4b8a-9c35-9e2f0a8b3c01";
const OTHER_PROJECT: &str = "7d3e9c52-4a6f-4b8a-9c35-9e2f0a8b3c02";
const NOT_A_PROJECT: &str = "7d3e9c52-4a6f-4b8a-9c35-9e2f0a8b3c03";

async fn executor() -> (GraphToolExecutor, Arc<NodeService>, TempDir) {
    let tmp = TempDir::new().unwrap();
    let mut store: Arc<SqliteStore> =
        Arc::new(SqliteStore::new(tmp.path().join("test.db")).await.unwrap());
    let ns = Arc::new(NodeService::new(&mut store).await.unwrap());

    for (id, node_type, title) in [
        (PROJECT, "project", "Query engine rewrite"),
        (OTHER_PROJECT, "project", "Billing cleanup"),
        (NOT_A_PROJECT, "task", "Cut the release branch"),
    ] {
        ns.create_node(Node::new_with_id(
            id.to_string(),
            node_type.to_string(),
            title.to_string(),
            json!({}),
        ))
        .await
        .unwrap();
    }

    let executor = GraphToolExecutor {
        node_service: Some(ns.clone()),
        embedding_service: Arc::new(RwLock::new(None)),
        inference_engine: None,
        playbook_lifecycle: None,
    };
    (executor, ns, tmp)
}

fn bare(result: &Value, key: &str) -> String {
    result[key]
        .as_str()
        .unwrap()
        .trim_start_matches("nodespace://")
        .to_string()
}

/// `(node_type, content)` of the children of `id`, in order.
async fn children(ns: &NodeService, id: &str) -> Vec<(String, String)> {
    ns.get_children(id)
        .await
        .unwrap()
        .into_iter()
        .map(|n| (n.node_type, n.content))
        .collect()
}

/// How many nodes of any of the three types the call writes exist.
async fn written_count(ns: &NodeService) -> usize {
    let mut total = 0;
    for node_type in ["task", "issue", "text", "checkbox"] {
        total += ns.query_nodes_by_type(node_type, false).await.unwrap().len();
    }
    total
}

#[tokio::test(flavor = "multi_thread")]
async fn one_call_writes_the_task_its_brief_its_checklist_and_its_project_link() {
    let (executor, ns, _tmp) = executor().await;
    // The "Cut the release branch" task seeded above is one task already.
    let before = written_count(&ns).await;

    let result = executor
        .execute(
            "create_task",
            json!({
                "title": "A done task with an unticked checklist item is reopened",
                "brief": [
                    "A task cannot stay done while a box directly under it is unticked.",
                    "Out of scope: tasks with no checklist."
                ],
                "checklist": [
                    "Marking a task done with an unticked box is refused",
                    "A task whose boxes are all ticked can be marked done"
                ],
                "project": "Query engine rewrite"
            }),
        )
        .await
        .expect("create_task must run");
    assert!(!result.is_error, "{:?}", result.result);

    let task = bare(&result.result, "id");
    assert_eq!(result.result["node_type"], "task");
    assert_eq!(result.result["brief_paragraphs"], 2);
    assert_eq!(result.result["checklist_items"], 2);
    assert_eq!(bare(&result.result, "project"), PROJECT);

    let node = ns.get_node(&task).await.unwrap().unwrap();
    assert_eq!(node.node_type, "task");
    assert_eq!(
        node.content,
        "A done task with an unticked checklist item is reopened"
    );

    // The brief first, then the checklist, in the order they were given, and
    // every box unticked.
    let under = children(&ns, &task).await;
    assert_eq!(
        under,
        vec![
            (
                "text".to_string(),
                "A task cannot stay done while a box directly under it is unticked.".to_string()
            ),
            (
                "text".to_string(),
                "Out of scope: tasks with no checklist.".to_string()
            ),
            (
                "checkbox".to_string(),
                "- [ ] Marking a task done with an unticked box is refused".to_string()
            ),
            (
                "checkbox".to_string(),
                "- [ ] A task whose boxes are all ticked can be marked done".to_string()
            ),
        ]
    );

    // In its project, and in no other.
    let in_project = executor
        .execute(
            "get_related_nodes",
            json!({ "id": PROJECT, "relationship_type": "tasks", "direction": "out" }),
        )
        .await
        .unwrap();
    let ids: Vec<String> = in_project.result["nodes"]
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
    assert_eq!(ids, [task.clone()]);

    // One task, two paragraphs, two boxes: five new nodes.
    assert_eq!(written_count(&ns).await, before + 5);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_change_to_code_is_an_issue_and_a_project_is_found_by_id() {
    let (executor, ns, _tmp) = executor().await;

    let result = executor
        .execute(
            "create_task",
            json!({
                "title": "An upload retries on a dropped connection",
                "brief": ["Uploads resume after a dropped connection."],
                "checklist": ["A dropped upload is retried up to three times"],
                "project": OTHER_PROJECT,
                "node_type": "issue"
            }),
        )
        .await
        .expect("create_task must run");
    assert!(!result.is_error, "{:?}", result.result);

    let issue = bare(&result.result, "id");
    assert_eq!(result.result["node_type"], "issue");
    assert_eq!(
        ns.get_node(&issue).await.unwrap().unwrap().node_type,
        "issue"
    );
    assert_eq!(bare(&result.result, "project"), OTHER_PROJECT);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_brief_and_checklist_sent_as_text_are_split_and_unmarked() {
    let (executor, ns, _tmp) = executor().await;

    let result = executor
        .execute(
            "create_task",
            json!({
                "title": "Exports keep their column order",
                "brief": "Exports keep the column order of the table.\n\nOut of scope: ordering by value.",
                "checklist": "- [ ] An export lists columns in table order\n* A reordered table exports reordered"
            }),
        )
        .await
        .expect("create_task must run");
    assert!(!result.is_error, "{:?}", result.result);
    assert!(result.result["project"].is_null());

    let under = children(&ns, &bare(&result.result, "id")).await;
    assert_eq!(
        under,
        vec![
            (
                "text".to_string(),
                "Exports keep the column order of the table.".to_string()
            ),
            (
                "text".to_string(),
                "Out of scope: ordering by value.".to_string()
            ),
            (
                "checkbox".to_string(),
                "- [ ] An export lists columns in table order".to_string()
            ),
            (
                "checkbox".to_string(),
                "- [ ] A reordered table exports reordered".to_string()
            ),
        ]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_task_with_no_brief_or_no_checklist_is_refused_with_nothing_created() {
    let (executor, ns, _tmp) = executor().await;
    let before = written_count(&ns).await;

    for args in [
        json!({ "title": "No brief", "brief": [], "checklist": ["It works"] }),
        json!({ "title": "No checklist", "brief": ["It does a thing."], "checklist": [] }),
        json!({ "title": "  ", "brief": ["It does a thing."], "checklist": ["It works"] }),
    ] {
        let error = executor
            .execute("create_task", args)
            .await
            .expect_err("an incomplete task must be refused");
        assert!(
            matches!(&error, ToolError::InvalidArguments { tool, .. } if tool == "create_task"),
            "{error:?}"
        );
    }
    assert_eq!(written_count(&ns).await, before);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_project_that_does_not_exist_or_is_not_one_refuses_with_nothing_created() {
    let (executor, ns, _tmp) = executor().await;
    let before = written_count(&ns).await;

    for project in [
        "7d3e9c52-4a6f-4b8a-9c35-ffffffffffff",
        "No such project",
        NOT_A_PROJECT,
    ] {
        let error = executor
            .execute(
                "create_task",
                json!({
                    "title": "A task",
                    "brief": ["It does a thing."],
                    "checklist": ["It works"],
                    "project": project
                }),
            )
            .await
            .expect_err("a bad project must be refused");
        let ToolError::UnresolvedId { tool, .. } = &error else {
            if matches!(&error, ToolError::InvalidArguments { tool, .. } if tool == "create_task") {
                continue;
            }
            panic!("unexpected error for {project}: {error:?}");
        };
        assert_eq!(tool, "create_task");
    }
    assert_eq!(written_count(&ns).await, before);
}
