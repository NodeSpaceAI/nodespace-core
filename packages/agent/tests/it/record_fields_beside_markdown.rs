//! A record read as markdown must reach the model with its fields.
//!
//! The markdown export writes a node's content and its descendants'. A
//! record's content is its title and its fields are properties, so the export
//! of a record is its title alone. `get_node` with `format=markdown` returns
//! the record's set fields beside that text, in the flat, storage-keyed map
//! its json format and `search_nodes` report.

use std::sync::Arc;

use nodespace_agent::local_agent::tools::GraphToolExecutor;
use nodespace_agent::AgentToolExecutor;
use nodespace_core::db::SqliteStore;
use nodespace_core::schema::handle_create_schema;
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

async fn create(executor: &GraphToolExecutor, args: Value) -> String {
    let created = executor
        .execute("create_node", args)
        .await
        .expect("create_node must succeed");
    assert!(!created.is_error, "{:?}", created.result);
    created.result["id"]
        .as_str()
        .unwrap()
        .trim_start_matches("nodespace://")
        .to_string()
}

async fn read_as_markdown(executor: &GraphToolExecutor, id: &str) -> Value {
    let got = executor
        .execute("get_node", json!({ "id": id, "format": "markdown" }))
        .await
        .expect("get_node must succeed");
    assert!(!got.is_error, "{:?}", got.result);
    got.result
}

#[tokio::test(flavor = "multi_thread")]
async fn a_record_read_as_markdown_carries_its_set_fields() {
    let (executor, ns, _tmp) = make_executor().await;
    handle_create_schema(
        &ns,
        json!({
            "name": "customer_account",
            "fields": [
                { "name": "signed_date", "type": "date" },
                { "name": "region", "type": "text" }
            ]
        }),
    )
    .await
    .expect("schema");
    let id = create(
        &executor,
        json!({
            "content": "Northwind Trading",
            "node_type": "customer_account",
            "field_values": { "signed_date": "2025-03-14" },
        }),
    )
    .await;

    let result = read_as_markdown(&executor, &id).await;

    assert!(
        result["markdown"]
            .as_str()
            .is_some_and(|md| md.contains("Northwind Trading")),
        "{result}"
    );
    assert_eq!(
        result["properties"]["signed_date"],
        json!("2025-03-14"),
        "the record's set field must be readable beside its text: {result}"
    );
    assert!(
        result["properties"].get("region").is_none(),
        "an unset field has no value to report: {result}"
    );
}

/// A core type's fields travel at the top level of the typed node. The model
/// reads them under the storage keys it writes with.
#[tokio::test(flavor = "multi_thread")]
async fn a_core_record_read_as_markdown_reports_fields_by_storage_key() {
    let (executor, _ns, _tmp) = make_executor().await;
    let id = create(
        &executor,
        json!({
            "content": "Renew the lease",
            "node_type": "task",
            "field_values": { "due_date": "2026-08-06" },
        }),
    )
    .await;

    let result = read_as_markdown(&executor, &id).await;

    assert_eq!(
        result["properties"]["due_date"],
        json!("2026-08-06"),
        "{result}"
    );
}

/// A document has no fields, and its result stays the text alone.
#[tokio::test(flavor = "multi_thread")]
async fn a_document_read_as_markdown_carries_no_properties() {
    let (executor, _ns, _tmp) = make_executor().await;
    let id = create(
        &executor,
        json!({ "content": "Retries back off.", "node_type": "text" }),
    )
    .await;

    let result = read_as_markdown(&executor, &id).await;

    assert!(result.get("markdown").is_some(), "{result}");
    assert!(result.get("properties").is_none(), "{result}");
}

/// A missing node gets the same answer whichever format was asked for.
#[tokio::test(flavor = "multi_thread")]
async fn a_missing_node_read_as_markdown_is_not_found() {
    let (executor, _ns, _tmp) = make_executor().await;
    let missing = "7d9f2c1e-0b4a-4e3f-9c8d-2a6b5e1f0c3d";

    let got = executor
        .execute("get_node", json!({ "id": missing, "format": "markdown" }))
        .await
        .expect("a missing node is a result for the model, not a tool failure");

    assert!(got.is_error, "{:?}", got.result);
    assert_eq!(
        got.result["error"],
        json!(format!("Node '{missing}' not found")),
        "{:?}",
        got.result
    );
}
