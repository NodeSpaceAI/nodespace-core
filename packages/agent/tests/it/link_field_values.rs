//! The write tools take a link field's value as its `{title, url}` object and
//! answer a bare string with the validation message.

use std::sync::Arc;

use nodespace_agent::local_agent::tools::GraphToolExecutor;
use nodespace_agent::AgentToolExecutor;
use nodespace_core::db::SqliteStore;
use nodespace_core::services::NodeService;
use serde_json::json;
use tempfile::TempDir;
use tokio::sync::RwLock;

async fn make_executor() -> (GraphToolExecutor, TempDir) {
    let tmp = TempDir::new().unwrap();
    let mut store: Arc<SqliteStore> =
        Arc::new(SqliteStore::new(tmp.path().join("test.db")).await.unwrap());
    let ns = Arc::new(NodeService::new(&mut store).await.unwrap());
    let executor = GraphToolExecutor {
        node_service: Some(ns),
        embedding_service: Arc::new(RwLock::new(None)),
        inference_engine: None,
        playbook_lifecycle: None,
    };
    (executor, tmp)
}

#[tokio::test(flavor = "multi_thread")]
async fn the_write_tools_take_a_link_and_refuse_a_bare_string() {
    let (executor, _tmp) = make_executor().await;
    let schema = executor
        .execute(
            "create_schema",
            json!({
                "name": "vendor",
                "fields": [
                    { "name": "website", "type": "link" },
                    { "name": "references", "type": "array", "itemType": "link" }
                ]
            }),
        )
        .await
        .expect("create_schema must run");
    assert!(!schema.is_error, "{:?}", schema.result);

    let link = json!({ "title": "Acme", "url": "https://acme.example" });
    let created = executor
        .execute(
            "create_node",
            json!({
                "content": "Acme",
                "node_type": "vendor",
                "field_values": { "website": link, "references": [link] },
            }),
        )
        .await
        .expect("create_node must run");
    assert!(!created.is_error, "{:?}", created.result);
    let id = created.result["id"].as_str().unwrap().to_string();

    let read = executor
        .execute("get_node", json!({ "id": id }))
        .await
        .expect("get_node must run");
    assert!(
        read.result.to_string().contains("https://acme.example"),
        "{:?}",
        read.result
    );

    for (tool, args) in [
        (
            "create_node",
            json!({
                "content": "Globex",
                "node_type": "vendor",
                "field_values": { "website": "https://globex.example" },
            }),
        ),
        (
            "update_node",
            json!({ "id": id, "field_values": { "website": "https://globex.example" } }),
        ),
    ] {
        let message = executor
            .execute(tool, args)
            .await
            .expect_err("a bare string is not a link")
            .to_string();
        assert!(
            message.contains("Link field 'website'") && message.contains("'title' and 'url'"),
            "{tool}: {message}"
        );
    }
}
