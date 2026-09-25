//! Regression test for `get_related_nodes` double-counting under `direction:
//! "both"` (also the tool's default).
//!
//! `"both"` fans out to one `rel_ops::get_related_nodes` call per direction. A
//! reverse relationship name (a declared `reverseName`, or a built-in inverse
//! like `child_of`) resolves to one traversal regardless of the requested
//! direction, so both calls returned the identical set and every related node
//! was reported twice. Drives the real `GraphToolExecutor::execute` surface
//! against a real `SqliteStore`.

use std::sync::Arc;

use nodespace_agent::local_agent::tools::GraphToolExecutor;
use nodespace_agent::AgentToolExecutor;
use nodespace_core::db::SqliteStore;
use nodespace_core::models::Node;
use nodespace_core::schema::handle_create_schema;
use nodespace_core::services::NodeService;
use serde_json::{json, Value};
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

async fn make_node(ns: &NodeService, id: &str, node_type: &str) {
    ns.create_node(Node::new_with_id(
        id.to_string(),
        node_type.to_string(),
        format!("{id} content"),
        json!({}),
    ))
    .await
    .unwrap();
}

/// `invoice.billed_to -> customer`, reversed as `invoices`, with two invoices
/// billed to one customer.
async fn seed_invoices(ns: &Arc<NodeService>) {
    handle_create_schema(
        ns,
        json!({
            "name": "Customer",
            "fields": [{ "name": "email", "type": "string", "protection": "user", "indexed": false }]
        }),
    )
    .await
    .unwrap();
    handle_create_schema(
        ns,
        json!({
            "name": "Invoice",
            "fields": [{ "name": "amount", "type": "number", "protection": "user", "indexed": false }],
            "relationships": [{
                "name": "billed_to",
                "targetType": "customer",
                "direction": "out",
                "cardinality": "one",
                "reverseName": "invoices",
                "reverseCardinality": "many"
            }]
        }),
    )
    .await
    .unwrap();

    make_node(ns, "c1", "customer").await;
    for inv in ["inv1", "inv2"] {
        make_node(ns, inv, "invoice").await;
        ns.create_relationship(inv, "billed_to", "c1", json!({}))
            .await
            .unwrap();
    }
}

fn ids(result: &Value) -> Vec<String> {
    let mut ids: Vec<String> = result["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|n| n["id"].as_str().unwrap().to_string())
        .collect();
    ids.sort();
    ids
}

#[tokio::test]
async fn reverse_name_with_both_direction_reports_each_node_once() {
    let (executor, ns, _tmp) = make_executor().await;
    seed_invoices(&ns).await;

    for args in [
        json!({ "id": "c1", "relationship_type": "invoices" }),
        json!({ "id": "c1", "relationship_type": "invoices", "direction": "both" }),
    ] {
        let result = executor
            .execute("get_related_nodes", args.clone())
            .await
            .unwrap();
        assert!(!result.is_error, "{args}: {:?}", result.result);
        assert_eq!(result.result["count"], 2, "{args}: {:?}", result.result);
        assert_eq!(
            ids(&result.result),
            vec!["nodespace://inv1", "nodespace://inv2"],
            "{args}"
        );
    }
}

/// A forward name still fans out to both directions: `both` must return the
/// union of the `out` and `in` results.
#[tokio::test]
async fn forward_name_with_both_direction_still_unions_directions() {
    let (executor, ns, _tmp) = make_executor().await;
    make_node(&ns, "a", "text").await;
    make_node(&ns, "b", "text").await;
    make_node(&ns, "c", "text").await;
    ns.create_relationship("a", "mentions", "b", json!({}))
        .await
        .unwrap();
    ns.create_relationship("c", "mentions", "a", json!({}))
        .await
        .unwrap();

    let result = executor
        .execute(
            "get_related_nodes",
            json!({ "id": "a", "relationship_type": "mentions", "direction": "both" }),
        )
        .await
        .unwrap();
    assert!(!result.is_error, "{:?}", result.result);
    assert_eq!(result.result["count"], 2, "{:?}", result.result);
    assert_eq!(ids(&result.result), vec!["nodespace://b", "nodespace://c"]);
}
