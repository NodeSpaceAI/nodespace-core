//! The `run_query` tool: a saved query run by id or title through the
//! production `GraphToolExecutor::execute` surface against a real store
//! (ADR-094 §1), and its place in the tool registry and the seeded skills.

use std::sync::Arc;

use nodespace_agent::local_agent::tools::{GraphToolExecutor, Tool};
use nodespace_agent::skill_pipeline::{seed_tool_nodes, SKILL_SEEDS};
use nodespace_agent::{AgentToolExecutor, ToolResult};
use nodespace_core::db::SqliteStore;
use nodespace_core::models::Node;
use nodespace_core::services::NodeService;
use serde_json::{json, Value};
use tempfile::TempDir;
use tokio::sync::RwLock;

async fn make_executor() -> (GraphToolExecutor, Arc<NodeService>, TempDir) {
    let tmp = TempDir::new().unwrap();
    let mut store: Arc<SqliteStore> =
        Arc::new(SqliteStore::new(tmp.path().join("test.db")).await.unwrap());
    let ns = Arc::new(NodeService::new(&mut store).await.unwrap());
    let executor = GraphToolExecutor {
        node_service: Some(ns.clone()),
        embedding_service: Arc::new(RwLock::new(None)),
        inference_engine: None,
        playbook_lifecycle: None,
    };
    (executor, ns, tmp)
}

async fn call(executor: &GraphToolExecutor, args: Value) -> ToolResult {
    executor
        .execute("run_query", args)
        .await
        .unwrap_or_else(|e| panic!("run_query must return a tool result: {e}"))
}

/// Three tasks and a saved query, "Not done", for the two that are open.
/// Returns the query's id.
async fn seed(ns: &Arc<NodeService>) -> String {
    for (content, properties) in [
        (
            "Write the spec",
            json!({ "status": "open", "priority": "low" }),
        ),
        (
            "Review the plan",
            json!({ "status": "open", "priority": "high" }),
        ),
        ("Ship it", json!({ "status": "done" })),
    ] {
        ns.create_node(Node::new(
            "task".to_string(),
            content.to_string(),
            properties,
        ))
        .await
        .expect("the task is created");
    }
    ns.create_node(Node::new(
        "query".to_string(),
        "Not done".to_string(),
        json!({
            "target_type": "task",
            "filters": [{
                "type": "property", "operator": "equals", "property": "status",
                "value": "done", "negate": true
            }],
            "sorting": [{ "field": "priority", "direction": "asc" }]
        }),
    ))
    .await
    .expect("the query is saved")
}

fn titles(result: &ToolResult) -> Vec<&str> {
    assert!(!result.is_error, "{}", result.result);
    let nodes = result.result["nodes"].as_array().expect("nodes");
    assert_eq!(result.result["count"], nodes.len());
    nodes
        .iter()
        .map(|node| node["title"].as_str().expect("a title"))
        .collect()
}

#[tokio::test]
async fn run_query_returns_what_a_saved_query_matches_by_id_or_title() {
    let (executor, ns, _tmp) = make_executor().await;
    let query_id = seed(&ns).await;
    let saved = ["Review the plan", "Write the spec"];

    let by_title = call(&executor, json!({ "query": "Not done" })).await;
    assert_eq!(titles(&by_title), saved);
    let by_id = call(&executor, json!({ "query": query_id })).await;
    assert_eq!(titles(&by_id), saved);
    // An id as the model reads it off an earlier result.
    let by_uri = call(
        &executor,
        json!({ "query": format!("nodespace://{query_id}") }),
    )
    .await;
    assert_eq!(titles(&by_uri), saved);

    // Each row carries the node's fields, as a search result does.
    assert_eq!(
        by_title.result["nodes"][0]["properties"]["priority"],
        "high"
    );
}

#[tokio::test]
async fn run_query_narrows_one_run_and_leaves_the_saved_query_as_it_was() {
    let (executor, ns, _tmp) = make_executor().await;
    let query_id = seed(&ns).await;
    let before = ns.get_node(&query_id).await.unwrap().unwrap();

    let narrowed = call(
        &executor,
        json!({ "query": "Not done", "filters": [{
            "type": "property", "operator": "equals", "property": "priority",
            "value": "high", "negate": true
        }] }),
    )
    .await;
    assert_eq!(titles(&narrowed), ["Write the spec"]);

    let limited = call(&executor, json!({ "query": "Not done", "limit": 1 })).await;
    assert_eq!(titles(&limited), ["Review the plan"]);
    // A full page says it may not be every match; a short one does not.
    assert!(
        limited.result["limit_reached"]
            .as_str()
            .is_some_and(|note| note.contains("first 1 matches")),
        "{}",
        limited.result
    );
    assert!(narrowed.result.get("limit_reached").is_none());
    // A limit of 0 is none given, not "return nothing".
    let zero = call(&executor, json!({ "query": "Not done", "limit": 0 })).await;
    assert_eq!(titles(&zero).len(), 2);

    let after = ns.get_node(&query_id).await.unwrap().unwrap();
    assert_eq!(after.version, before.version);
    assert_eq!(after.properties, before.properties);
    assert_eq!(
        titles(&call(&executor, json!({ "query": "Not done" })).await).len(),
        2
    );
}

#[tokio::test]
async fn run_query_says_when_a_title_names_no_query_or_several() {
    let (executor, ns, _tmp) = make_executor().await;
    seed(&ns).await;
    let twin = ns
        .create_node(Node::new(
            "query".to_string(),
            "not DONE".to_string(),
            json!({ "target_type": "task", "filters": [] }),
        ))
        .await
        .unwrap();

    let error = |result: ToolResult| {
        assert!(result.is_error, "expected a tool error: {}", result.result);
        result.result["error"]
            .as_str()
            .expect("an error message")
            .to_string()
    };

    let missing = error(call(&executor, json!({ "query": "Triage" })).await);
    assert!(
        missing.contains("no saved query has the id or title 'Triage'"),
        "{missing}"
    );

    let several = error(call(&executor, json!({ "query": "Not done" })).await);
    assert!(
        several.contains("2 saved queries are titled 'Not done'") && several.contains(&twin),
        "{several}"
    );
}

/// The tool is in the registry with the command that does the same from a
/// shell, its seeded node records that command, and a seeded skill offers it.
#[test]
fn run_query_is_registered_seeded_and_offered() {
    let tool = Tool::from_name("run_query").expect("run_query is in the registry");
    assert_eq!(tool.cli_command(), Some("nodespace query run"));
    assert!(!tool.is_write());

    let seeded = seed_tool_nodes()
        .into_iter()
        .find(|seed| seed.title == "run_query")
        .expect("run_query has a seeded tool node");
    assert_eq!(seeded.root_properties["cli_command"], "nodespace query run");

    assert!(
        SKILL_SEEDS
            .iter()
            .any(|skill| skill.tools.contains(&"run_query")),
        "no seeded skill offers run_query, so the built-in agent could never call it"
    );

    // A run-time filter can be negated; the item is otherwise search_nodes's.
    let definition = nodespace_agent::local_agent::tools::all_tool_definitions()
        .into_iter()
        .find(|definition| definition.name == "run_query")
        .expect("a definition");
    let item = &definition.parameters_schema["properties"]["filters"]["items"];
    assert_eq!(item["properties"]["negate"]["type"], "boolean");
    assert!(item["properties"]["operator"]["enum"].is_array());
    assert_eq!(definition.parameters_schema["required"], json!(["query"]));
}
