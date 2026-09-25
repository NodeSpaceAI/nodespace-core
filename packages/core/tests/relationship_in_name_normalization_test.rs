//! A relationship written through an `in`-direction declaration's own name is
//! stored as the forward edge it is the far end's view of.
//!
//! Edges are stored once: `relationship_type` is the forward (`out`) name,
//! `in_node` the source, `out_node` the target. `adr` declares `supersedes`
//! (out) and `superseded_by` (in, `reverseName: supersedes`), so
//! `old --superseded_by--> new` is `new --supersedes--> old`. Every write path
//! (create, update, delete) rewrites the `in` spelling to that forward shape,
//! so one logical edge has one storage shape and the forward declaration's
//! cardinality applies to it.

use anyhow::Result;
use nodespace_core::{
    db::SqliteStore,
    models::Node,
    ops::rel_ops::{self, GetRelatedInput},
    schema::handle_create_schema,
    services::NodeService,
};
use serde_json::json;
use std::sync::Arc;
use tempfile::TempDir;

async fn create_test_service() -> Result<(Arc<NodeService>, TempDir)> {
    let temp_dir = TempDir::new()?;
    let db_path = temp_dir.path().join("test.db");
    let mut store = Arc::new(SqliteStore::new(db_path).await?);
    let node_service = Arc::new(NodeService::new(&mut store).await?);
    Ok((node_service, temp_dir))
}

async fn make_adr(svc: &NodeService, id: &str) -> Result<()> {
    svc.create_node(Node::new_with_id(
        id.to_string(),
        "in_norm_adr".to_string(),
        format!("{id} content"),
        json!({}),
    ))
    .await?;
    Ok(())
}

/// `supersedes` is one-to-one: an ADR supersedes at most one other, and is
/// superseded by at most one. The pair carries an enum edge field so the
/// update path's validation is observable.
async fn create_adr_schema(svc: &Arc<NodeService>) -> Result<()> {
    handle_create_schema(
        svc,
        json!({
            "name": "in_norm_adr",
            "fields": [],
            "relationships": [
                {
                    "name": "supersedes",
                    "targetType": "in_norm_adr",
                    "direction": "out",
                    "cardinality": "one",
                    "reverseName": "superseded_by",
                    "reverseCardinality": "one",
                    "edgeFields": [{
                        "name": "reason",
                        "type": "enum",
                        "coreValues": [
                            { "value": "revised", "label": "Revised" },
                            { "value": "reversed", "label": "Reversed" }
                        ]
                    }]
                },
                {
                    "name": "superseded_by",
                    "targetType": "in_norm_adr",
                    "direction": "in",
                    "cardinality": "one",
                    "reverseName": "supersedes",
                    "reverseCardinality": "one"
                }
            ]
        }),
    )
    .await
    .map_err(|e| anyhow::anyhow!("adr schema: {e}"))?;
    Ok(())
}

/// Every stored edge leaving one of `ids` under either spelling, as
/// `(in_node, out_node, relationship_type)`. `get_related_nodes` queries the
/// store by the literal `relationship_type`, so this sees the raw rows.
async fn stored_edges(svc: &NodeService, ids: &[&str]) -> Result<Vec<(String, String, String)>> {
    let mut stored = Vec::new();
    for id in ids {
        for name in ["supersedes", "superseded_by"] {
            for target in related_ids(svc, id, name, "out").await? {
                stored.push((id.to_string(), target, name.to_string()));
            }
        }
    }
    stored.sort();
    Ok(stored)
}

async fn related_ids(svc: &NodeService, id: &str, name: &str, dir: &str) -> Result<Vec<String>> {
    Ok(svc
        .get_related_nodes(id, name, dir)
        .await?
        .into_iter()
        .map(|n| n.id)
        .collect())
}

#[tokio::test]
async fn write_through_in_name_stores_the_forward_edge() -> Result<()> {
    let (svc, _t) = create_test_service().await?;
    create_adr_schema(&svc).await?;
    make_adr(&svc, "old").await?;
    make_adr(&svc, "new").await?;

    svc.create_relationship("old", "superseded_by", "new", json!({}))
        .await?;

    assert_eq!(
        stored_edges(&svc, &["old", "new"]).await?,
        vec![("new".into(), "old".into(), "supersedes".into())],
        "same row `create_relationship(new, supersedes, old)` stores"
    );
    assert_eq!(
        related_ids(&svc, "new", "supersedes", "out").await?,
        ["old"]
    );
    assert_eq!(related_ids(&svc, "old", "supersedes", "in").await?, ["new"]);

    // Writing the forward spelling of the same edge is an idempotent no-op.
    svc.create_relationship("new", "supersedes", "old", json!({}))
        .await?;
    assert_eq!(stored_edges(&svc, &["old", "new"]).await?.len(), 1);
    Ok(())
}

/// `reverse_cardinality: one` on `supersedes` means `old` is superseded by at
/// most one ADR. A second write through the `in` name replaces the first
/// superseder rather than adding a second edge.
#[tokio::test]
async fn write_through_in_name_enforces_reverse_cardinality() -> Result<()> {
    let (svc, _t) = create_test_service().await?;
    create_adr_schema(&svc).await?;
    for id in ["old", "new1", "new2"] {
        make_adr(&svc, id).await?;
    }

    svc.create_relationship("old", "superseded_by", "new1", json!({}))
        .await?;
    svc.create_relationship("old", "superseded_by", "new2", json!({}))
        .await?;

    assert_eq!(
        stored_edges(&svc, &["old", "new1", "new2"]).await?,
        vec![("new2".into(), "old".into(), "supersedes".into())]
    );
    Ok(())
}

/// `cardinality: one` on `supersedes` means `new` supersedes at most one ADR.
/// A write through the `in` name from a second target replaces `new`'s prior
/// forward edge.
#[tokio::test]
async fn write_through_in_name_enforces_forward_cardinality() -> Result<()> {
    let (svc, _t) = create_test_service().await?;
    create_adr_schema(&svc).await?;
    for id in ["old1", "old2", "new"] {
        make_adr(&svc, id).await?;
    }

    svc.create_relationship("new", "supersedes", "old1", json!({}))
        .await?;
    svc.create_relationship("old2", "superseded_by", "new", json!({}))
        .await?;

    assert_eq!(
        stored_edges(&svc, &["old1", "old2", "new"]).await?,
        vec![("new".into(), "old2".into(), "supersedes".into())]
    );
    Ok(())
}

#[tokio::test]
async fn update_and_delete_resolve_the_in_name() -> Result<()> {
    let (svc, _t) = create_test_service().await?;
    create_adr_schema(&svc).await?;
    make_adr(&svc, "old").await?;
    make_adr(&svc, "new").await?;

    svc.create_relationship("old", "superseded_by", "new", json!({}))
        .await?;

    // The forward declaration's edge fields validate an update through the
    // `in` name, and the update lands on the forward row.
    let bad = svc
        .update_relationship_properties("old", "superseded_by", "new", json!({"reason": "nope"}))
        .await;
    assert!(bad.is_err(), "enum edge field must be validated");
    svc.update_relationship_properties("old", "superseded_by", "new", json!({"reason": "revised"}))
        .await?;
    let edges = svc
        .get_related_nodes_with_edges("new", "supersedes", "out")
        .await?;
    assert_eq!(edges.len(), 1);
    assert_eq!(edges[0].1["reason"], "revised");

    svc.delete_relationship("old", "superseded_by", "new")
        .await?;
    assert!(stored_edges(&svc, &["old", "new"]).await?.is_empty());
    Ok(())
}

/// A forward-written edge is removable through the `in` name too — the two
/// spellings address one edge.
#[tokio::test]
async fn forward_written_edge_deletes_through_in_name() -> Result<()> {
    let (svc, _t) = create_test_service().await?;
    create_adr_schema(&svc).await?;
    make_adr(&svc, "old").await?;
    make_adr(&svc, "new").await?;

    svc.create_relationship("new", "supersedes", "old", json!({}))
        .await?;
    svc.delete_relationship("old", "superseded_by", "new")
        .await?;

    assert!(stored_edges(&svc, &["old", "new"]).await?.is_empty());
    Ok(())
}

/// Reading through the node's own `in` declaration traverses the forward
/// edge inbound, whichever spelling wrote it.
#[tokio::test]
async fn read_through_in_name_finds_the_forward_edge() -> Result<()> {
    let (svc, _t) = create_test_service().await?;
    create_adr_schema(&svc).await?;
    make_adr(&svc, "old").await?;
    make_adr(&svc, "new").await?;

    svc.create_relationship("new", "supersedes", "old", json!({}))
        .await?;

    let out = rel_ops::get_related_nodes(
        &svc,
        GetRelatedInput {
            node_id: "old".to_string(),
            relationship_name: "superseded_by".to_string(),
            direction: "out".to_string(),
        },
    )
    .await?;
    assert_eq!(out.relationship_name, "supersedes");
    assert_eq!(out.direction, "in");
    assert_eq!(out.count, 1);
    assert_eq!(out.related_nodes[0]["id"], "new");
    Ok(())
}
