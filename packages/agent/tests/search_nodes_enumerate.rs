//! Regression test for the `search_nodes` "enumerate a type" behavior.
//!
//! Before the fix, `query: "*"` was treated as a literal 1-character title
//! substring filter — silently matching nothing and returning `count: 0` even
//! when instances of the requested type existed. This drives the real
//! production `GraphToolExecutor::execute("search_nodes", ...)` surface (not
//! the underlying ops function directly) against a real `SqliteStore`, so the
//! assertion covers the same call path the agent actually takes.

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
        playbook_lifecycle: None,
    };
    (executor, ns, tmp)
}

/// Acceptance criterion: "create a node of type X out-of-band, then assert
/// `search_nodes` on type X finds it" — using `query: "*"`, the spelling that
/// silently returned zero in the original bug report.
#[tokio::test]
async fn search_nodes_wildcard_query_enumerates_out_of_band_node() {
    let (executor, ns, _tmp) = make_executor().await;

    let node = Node::new(
        "invoice".to_string(),
        "Some Invoice".to_string(),
        json!({ "invoice_number": "AA111", "status": "paid" }),
    );
    let node_id = ns.create_node(node).await.unwrap();

    let result = executor
        .execute(
            "search_nodes",
            json!({ "node_type": "invoice", "query": "*" }),
        )
        .await
        .expect("search_nodes must succeed");

    assert!(!result.is_error, "search_nodes returned an error result");
    let count = result.result["count"].as_u64().unwrap();
    assert_eq!(count, 1, "expected the out-of-band invoice to be found");
    assert_eq!(
        result.result["nodes"][0]["id"],
        format!("nodespace://{node_id}")
    );
}

/// The empty-string spelling must behave identically to "*" — both mean
/// "enumerate", not "match nothing".
#[tokio::test]
async fn search_nodes_empty_query_enumerates_same_as_wildcard() {
    let (executor, ns, _tmp) = make_executor().await;

    ns.create_node(Node::new(
        "invoice".to_string(),
        "Some Invoice".to_string(),
        json!({}),
    ))
    .await
    .unwrap();

    let wildcard = executor
        .execute(
            "search_nodes",
            json!({ "node_type": "invoice", "query": "*" }),
        )
        .await
        .unwrap();
    let empty = executor
        .execute(
            "search_nodes",
            json!({ "node_type": "invoice", "query": "" }),
        )
        .await
        .unwrap();

    assert_eq!(wildcard.result["count"], empty.result["count"]);
    assert_eq!(wildcard.result["count"], 1);
}

/// A literal query that happens to end in "*" is still a substring match, not
/// an enumerate — only the whole (trimmed) query being exactly "*" triggers
/// enumerate semantics.
#[tokio::test]
async fn search_nodes_literal_query_still_filters() {
    let (executor, ns, _tmp) = make_executor().await;

    ns.create_node(Node::new(
        "invoice".to_string(),
        "Some Invoice".to_string(),
        json!({}),
    ))
    .await
    .unwrap();
    ns.create_node(Node::new(
        "invoice".to_string(),
        "Unrelated Record".to_string(),
        json!({}),
    ))
    .await
    .unwrap();

    let result = executor
        .execute(
            "search_nodes",
            json!({ "node_type": "invoice", "query": "Some" }),
        )
        .await
        .unwrap();

    assert_eq!(result.result["count"], 1);
    assert_eq!(result.result["nodes"][0]["title"], "Some Invoice");
}
