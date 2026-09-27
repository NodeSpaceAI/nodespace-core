//! `node_ops` read results must carry an extending node's inherited values
//! (ADR-078). They live in the ancestor's property bucket, and the wire
//! flattener reads one bucket, so each result must have the chain collapsed
//! first — the collapse the daemon applies at its own boundary. The in-process
//! agent reads through `node_ops` directly and has no such boundary.

use anyhow::Result;
use nodespace_core::{
    db::SqliteStore,
    models::Node,
    ops::node_ops::{get_node, query_nodes, GetNodeInput, QueryNodesInput},
    schema::handle_create_schema,
    services::NodeService,
};
use serde_json::json;
use std::sync::Arc;
use tempfile::TempDir;

async fn service_with_refund_node() -> Result<(Arc<NodeService>, TempDir)> {
    let temp_dir = TempDir::new()?;
    let mut store = Arc::new(SqliteStore::new(temp_dir.path().join("test.db")).await?);
    let svc = Arc::new(NodeService::new(&mut store).await?);

    handle_create_schema(
        &svc,
        json!({ "name": "ledger_entry", "fields": [{ "name": "amount", "type": "number" }] }),
    )
    .await
    .map_err(|e| anyhow::anyhow!("base schema: {e}"))?;
    handle_create_schema(
        &svc,
        json!({
            "name": "refund_entry",
            "extends": "ledger_entry",
            "fields": [{ "name": "reason", "type": "string" }]
        }),
    )
    .await
    .map_err(|e| anyhow::anyhow!("subtype schema: {e}"))?;

    svc.create_node(Node::new_with_id(
        "refund-1".to_string(),
        "refund_entry".to_string(),
        "Refund".to_string(),
        json!({ "amount": 42, "reason": "duplicate" }),
    ))
    .await?;
    Ok((svc, temp_dir))
}

#[tokio::test]
async fn get_node_returns_an_extending_nodes_inherited_values() -> Result<()> {
    let (svc, _t) = service_with_refund_node().await?;

    let node = get_node(
        &svc,
        GetNodeInput {
            node_id: "refund-1".to_string(),
        },
    )
    .await?;

    assert_eq!(node["properties"]["amount"], json!(42), "{node}");
    assert_eq!(node["properties"]["reason"], json!("duplicate"), "{node}");
    Ok(())
}

#[tokio::test]
async fn query_nodes_returns_an_extending_nodes_inherited_values() -> Result<()> {
    let (svc, _t) = service_with_refund_node().await?;

    let out = query_nodes(
        &svc,
        QueryNodesInput {
            node_type: Some("refund_entry".to_string()),
            limit: None,
            offset: None,
            collection_id: None,
            collection: None,
            filters: None,
        },
    )
    .await?;

    assert_eq!(out.count, 1);
    assert_eq!(
        out.nodes[0]["properties"]["amount"],
        json!(42),
        "{:?}",
        out.nodes
    );
    Ok(())
}
