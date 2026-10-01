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

// The nodes these tests create. Every node has a UUID, and the ids are
// ordered so a sorted result reads in the order the test names them.
const CUSTOMER: &str = "3a6f1d52-0c4e-4b7a-9f21-5d8e7c6b4a01";
const INVOICE_1: &str = "3a6f1d52-0c4e-4b7a-9f21-5d8e7c6b4a02";
const INVOICE_2: &str = "3a6f1d52-0c4e-4b7a-9f21-5d8e7c6b4a03";
const PARENT: &str = "3a6f1d52-0c4e-4b7a-9f21-5d8e7c6b4a04";
const CHILD: &str = "3a6f1d52-0c4e-4b7a-9f21-5d8e7c6b4a05";
const NODE_A: &str = "3a6f1d52-0c4e-4b7a-9f21-5d8e7c6b4a06";
const NODE_B: &str = "3a6f1d52-0c4e-4b7a-9f21-5d8e7c6b4a07";
const NODE_C: &str = "3a6f1d52-0c4e-4b7a-9f21-5d8e7c6b4a08";

/// The `nodespace://` URI a tool result reports for a node.
fn uri(id: &str) -> String {
    format!("nodespace://{id}")
}

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
            "fields": [{ "name": "email", "type": "text", "protection": "user", "indexed": false }]
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

    make_node(ns, CUSTOMER, "customer").await;
    for inv in [INVOICE_1, INVOICE_2] {
        make_node(ns, inv, "invoice").await;
        ns.create_relationship(inv, "billed_to", CUSTOMER, json!({}))
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
        json!({ "id": CUSTOMER, "relationship_type": "invoices" }),
        json!({ "id": CUSTOMER, "relationship_type": "invoices", "direction": "both" }),
    ] {
        let result = executor
            .execute("get_related_nodes", args.clone())
            .await
            .unwrap();
        assert!(!result.is_error, "{args}: {:?}", result.result);
        assert_eq!(result.result["count"], 2, "{args}: {:?}", result.result);
        assert_eq!(
            ids(&result.result),
            vec![uri(INVOICE_1), uri(INVOICE_2)],
            "{args}"
        );
        // Each hit is labelled with the traversal that actually ran.
        for node in result.result["nodes"].as_array().unwrap() {
            assert_eq!(node["direction"], "in", "{args}");
            assert_eq!(node["relationship_type"], "billed_to", "{args}");
        }
    }
}

/// A `reverseName` spelled the same as its forward name comes back
/// un-rewritten, so the fan-out must not rely on the name changing to spot a
/// reverse resolution.
#[tokio::test]
async fn reverse_name_equal_to_forward_name_reports_each_node_once() {
    let (executor, ns, _tmp) = make_executor().await;
    handle_create_schema(
        &ns,
        json!({
            "name": "Customer",
            "fields": [{ "name": "email", "type": "text", "protection": "user", "indexed": false }]
        }),
    )
    .await
    .unwrap();
    handle_create_schema(
        &ns,
        json!({
            "name": "Invoice",
            "fields": [{ "name": "amount", "type": "number", "protection": "user", "indexed": false }],
            "relationships": [{
                "name": "related",
                "targetType": "customer",
                "direction": "out",
                "cardinality": "many",
                "reverseName": "related",
                "reverseCardinality": "many"
            }]
        }),
    )
    .await
    .unwrap();
    make_node(&ns, CUSTOMER, "customer").await;
    for inv in [INVOICE_1, INVOICE_2] {
        make_node(&ns, inv, "invoice").await;
        ns.create_relationship(inv, "related", CUSTOMER, json!({}))
            .await
            .unwrap();
    }

    let result = executor
        .execute(
            "get_related_nodes",
            json!({ "id": CUSTOMER, "relationship_type": "related" }),
        )
        .await
        .unwrap();
    assert!(!result.is_error, "{:?}", result.result);
    assert_eq!(result.result["count"], 2, "{:?}", result.result);
    assert_eq!(ids(&result.result), vec![uri(INVOICE_1), uri(INVOICE_2)]);
}

/// A built-in inverse (`child_of`) resolves through a different branch than a
/// schema-declared `reverseName` and must be reported once too.
#[tokio::test]
async fn builtin_reverse_name_with_both_direction_reports_each_node_once() {
    let (executor, ns, _tmp) = make_executor().await;
    make_node(&ns, PARENT, "text").await;
    make_node(&ns, CHILD, "text").await;
    ns.create_relationship(PARENT, "has_child", CHILD, json!({}))
        .await
        .unwrap();

    let result = executor
        .execute(
            "get_related_nodes",
            json!({ "id": CHILD, "relationship_type": "child_of" }),
        )
        .await
        .unwrap();
    assert!(!result.is_error, "{:?}", result.result);
    assert_eq!(result.result["count"], 1, "{:?}", result.result);
    assert_eq!(ids(&result.result), vec![uri(PARENT)]);
    assert_eq!(result.result["nodes"][0]["relationship_type"], "has_child");
    assert_eq!(result.result["nodes"][0]["direction"], "in");
}

/// A forward name still fans out to both directions: `both` must return the
/// union of the `out` and `in` results.
#[tokio::test]
async fn forward_name_with_both_direction_still_unions_directions() {
    let (executor, ns, _tmp) = make_executor().await;
    make_node(&ns, NODE_A, "text").await;
    make_node(&ns, NODE_B, "text").await;
    make_node(&ns, NODE_C, "text").await;
    ns.create_relationship(NODE_A, "mentions", NODE_B, json!({}))
        .await
        .unwrap();
    ns.create_relationship(NODE_C, "mentions", NODE_A, json!({}))
        .await
        .unwrap();

    let result = executor
        .execute(
            "get_related_nodes",
            json!({ "id": NODE_A, "relationship_type": "mentions", "direction": "both" }),
        )
        .await
        .unwrap();
    assert!(!result.is_error, "{:?}", result.result);
    assert_eq!(result.result["count"], 2, "{:?}", result.result);
    assert_eq!(ids(&result.result), vec![uri(NODE_B), uri(NODE_C)]);
}
