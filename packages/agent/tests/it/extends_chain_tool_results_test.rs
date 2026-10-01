//! The agent's node tool results must describe an extending type (ADR-078)
//! across its whole chain: `get_node` returns inherited values and lists
//! inherited fields in `available_properties`, and an empty type-scoped
//! `search_nodes` names inherited fields in `filterable_properties`.
//!
//! A subtype's inherited values are stored in the ancestor's property bucket
//! and its inherited fields are declared on the ancestor's schema, so a read
//! of the node's own bucket or schema alone tells the model the subtype lacks
//! them — and it then declines to set or filter on fields that exist.

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

/// `ledger_entry` declares `amount` and `memo`; `refund_entry` extends it,
/// declaring only `reason`.
async fn create_base_and_subtype(ns: &Arc<NodeService>) {
    handle_create_schema(
        ns,
        json!({
            "name": "ledger_entry",
            "fields": [
                { "name": "amount", "type": "number" },
                { "name": "memo", "type": "text" }
            ]
        }),
    )
    .await
    .expect("base schema");
    handle_create_schema(
        ns,
        json!({
            "name": "refund_entry",
            "extends": "ledger_entry",
            "fields": [{ "name": "reason", "type": "text" }]
        }),
    )
    .await
    .expect("subtype schema");
}

fn field<'a>(list: &'a Value, name: &str) -> Option<&'a Value> {
    list.as_array()?.iter().find(|f| f["name"] == name)
}

#[tokio::test(flavor = "multi_thread")]
async fn get_node_on_a_subtype_returns_and_lists_inherited_fields() {
    let (executor, ns, _tmp) = make_executor().await;
    create_base_and_subtype(&ns).await;

    let created = executor
        .execute(
            "create_node",
            json!({
                "content": "Refund for a duplicate charge",
                "node_type": "refund_entry",
                "field_values": { "amount": 42, "reason": "duplicate" },
            }),
        )
        .await
        .expect("create_node must succeed");
    let id = created.result["id"]
        .as_str()
        .unwrap()
        .trim_start_matches("nodespace://")
        .to_string();

    let got = executor
        .execute("get_node", json!({ "id": id }))
        .await
        .expect("get_node must succeed");
    let result = &got.result;

    assert_eq!(
        result["properties"]["amount"],
        json!(42),
        "the inherited value lives in the ancestor's bucket and must still be returned: {result}"
    );
    assert_eq!(result["properties"]["reason"], json!("duplicate"));

    let available = &result["available_properties"];
    let amount = field(available, "amount")
        .unwrap_or_else(|| panic!("inherited `amount` must be listed: {available}"));
    assert_eq!(amount["set"], json!(true), "{available}");
    let memo = field(available, "memo")
        .unwrap_or_else(|| panic!("inherited unset `memo` must be listed: {available}"));
    assert_eq!(memo["set"], json!(false), "{available}");
    assert_eq!(field(available, "reason").unwrap()["set"], json!(true));
}

#[tokio::test(flavor = "multi_thread")]
async fn empty_search_on_a_subtype_names_inherited_filterable_fields() {
    let (executor, ns, _tmp) = make_executor().await;
    create_base_and_subtype(&ns).await;

    let found = executor
        .execute(
            "search_nodes",
            json!({ "query": "", "node_type": "refund_entry" }),
        )
        .await
        .expect("search_nodes must succeed");

    assert_eq!(found.result["count"], json!(0));
    let fields = &found.result["filterable_properties"];
    for name in ["reason", "amount", "memo"] {
        assert!(
            field(fields, name).is_some(),
            "`{name}` must be filterable on the subtype: {}",
            found.result
        );
    }
}
