//! A built-in relationship written to a node that does not exist is refused as
//! a missing node, not left to the store's foreign-key failure.

use anyhow::Result;
use nodespace_core::{
    db::SqliteStore,
    models::Node,
    ops::{rel_ops, OpsError},
    services::NodeService,
};
use serde_json::json;
use std::sync::Arc;
use tempfile::TempDir;

const REAL: &str = "8160b75c-ba44-5791-ae15-8f95d5a891f0";
const MISSING: &str = "11111111-2222-3333-4444-555555555555";

async fn service_with_one_node() -> Result<(Arc<NodeService>, TempDir)> {
    let temp_dir = TempDir::new()?;
    let mut store = Arc::new(SqliteStore::new(temp_dir.path().join("test.db")).await?);
    let svc = Arc::new(NodeService::new(&mut store).await?);
    svc.create_node(Node::new_with_id(
        REAL.to_string(),
        "text".to_string(),
        "Real".to_string(),
        json!({}),
    ))
    .await?;
    Ok((svc, temp_dir))
}

async fn mentions(svc: &Arc<NodeService>, from: &str, to: &str) -> Result<(), OpsError> {
    rel_ops::create_relationship(
        svc,
        rel_ops::CreateRelInput {
            source_id: from.to_string(),
            relationship_name: "mentions".to_string(),
            target_id: to.to_string(),
            edge_data: None,
        },
    )
    .await
    .map(|_| ())
}

#[tokio::test]
async fn a_mentions_edge_to_a_missing_target_is_a_missing_node() -> Result<()> {
    let (svc, _t) = service_with_one_node().await?;
    let err = mentions(&svc, REAL, MISSING).await.unwrap_err();
    assert!(
        matches!(&err, OpsError::NotFound { id } if id == MISSING),
        "got {err:?}"
    );
    Ok(())
}

#[tokio::test]
async fn a_mentions_edge_from_a_missing_source_is_a_missing_node() -> Result<()> {
    let (svc, _t) = service_with_one_node().await?;
    let err = mentions(&svc, MISSING, REAL).await.unwrap_err();
    assert!(
        matches!(&err, OpsError::NotFound { id } if id == MISSING),
        "got {err:?}"
    );
    Ok(())
}
