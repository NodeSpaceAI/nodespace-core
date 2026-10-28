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
    link(svc, from, "mentions", to).await
}

async fn link(svc: &Arc<NodeService>, from: &str, name: &str, to: &str) -> Result<(), OpsError> {
    rel_ops::create_relationship(
        svc,
        rel_ops::CreateRelInput {
            source_id: from.to_string(),
            relationship_name: name.to_string(),
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

#[tokio::test]
async fn a_has_role_edge_to_a_missing_target_is_a_missing_node() -> Result<()> {
    let (svc, _t) = service_with_one_node().await?;
    let err = link(&svc, REAL, "has_role", MISSING).await.unwrap_err();
    assert!(
        matches!(&err, OpsError::NotFound { id } if id == MISSING),
        "got {err:?}"
    );
    Ok(())
}

#[tokio::test]
async fn a_mentions_edge_between_existing_nodes_is_created() -> Result<()> {
    let (svc, _t) = service_with_one_node().await?;
    let other = "bf53417d-8511-5995-aecb-300ecf227fcb";
    svc.create_node(Node::new_with_id(
        other.to_string(),
        "text".to_string(),
        "Other".to_string(),
        json!({}),
    ))
    .await?;
    mentions(&svc, REAL, other).await?;
    Ok(())
}

/// A date page that has no row yet is a real target: the edge is written and
/// the page is saved with it, since the edge's foreign key needs the row.
#[tokio::test]
async fn a_mentions_edge_to_an_unvisited_date_page_saves_the_page() -> Result<()> {
    let (svc, _t) = service_with_one_node().await?;
    let date = "2026-10-24";
    assert!(svc.store().get_node(date).await?.is_none());

    mentions(&svc, REAL, date).await?;

    assert!(
        svc.store().get_node(date).await?.is_some(),
        "the date page is saved"
    );
    assert!(
        svc.store()
            .relationship_exists(REAL, date, "mentions")
            .await?
    );
    Ok(())
}
