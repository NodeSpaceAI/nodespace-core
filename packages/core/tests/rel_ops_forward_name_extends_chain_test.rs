//! `resolve_relationship_name`'s forward-name check, against a subtype
//! reached only through the ADR-078 `extends` chain.
//!
//! `resolve_relationship_name`'s forward-name lookup used to consult
//! `node_type`'s own schema directly (`get_schema_node`), never the
//! `extends`-chain-merged set `NodeService::resolve_relationships` provides.
//! A relationship declared `Forward` only on an ancestor schema and inherited
//! (not redeclared) by a subtype was invisible to it -- resolution fell
//! through to the reverse-name checks, found nothing there either, and
//! returned `OpsError::InvalidParams`, the "undeclared name" error.
//!
//! The write path (`create_relationship`, via `resolve_declared_relationship`)
//! was already `extends`-chain aware, so a subtype instance could carry a
//! real edge that this read-side resolver refused to recognize as declared in
//! either direction at all -- distinct from (and deeper than) a cardinality
//! misclassification: the fetch itself never found the row. Per
//! `resolve_relationship_name`'s own doc comment this resolution is shared
//! with the CLI's read path and with `GraphResolver::fetch_related_nodes`
//! (which treats `InvalidParams` here as "undeclared, not a failure" and
//! silently returns an empty result), so the bug reached both.
//!
//! These tests cover the fix directly at the `rel_ops` layer: the forward
//! name resolves as `Forward` (not an error) for a subtype instance, and a
//! real edge attached to that instance is actually readable back through
//! `get_related_nodes`, not just correctly classified.

use anyhow::Result;
use nodespace_core::{
    db::SqliteStore,
    models::Node,
    ops::rel_ops::{self, ResolvedRelName},
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

async fn make_node(svc: &NodeService, id: &str, node_type: &str) -> Result<()> {
    svc.create_node(Node::new_with_id(
        id.to_string(),
        node_type.to_string(),
        format!("{id} content"),
        json!({}),
    ))
    .await?;
    Ok(())
}

/// Base type declares `story` forward, out to `rel_ops_ext_target`; the
/// subtype extends the base without redeclaring it.
async fn create_base_and_subtype(svc: &Arc<NodeService>) -> Result<()> {
    handle_create_schema(
        svc,
        json!({
            "name": "rel_ops_ext_target",
            "fields": [{ "name": "title", "type": "string", "protection": "user", "indexed": false }]
        }),
    )
    .await
    .map_err(|e| anyhow::anyhow!("target schema: {e}"))?;

    handle_create_schema(
        svc,
        json!({
            "name": "rel_ops_ext_base",
            "fields": [],
            "relationships": [{
                "name": "story",
                "targetType": "rel_ops_ext_target",
                "direction": "out",
                "cardinality": "one",
                "reverseName": "tasks",
                "reverseCardinality": "many"
            }]
        }),
    )
    .await
    .map_err(|e| anyhow::anyhow!("base schema: {e}"))?;

    handle_create_schema(
        svc,
        json!({
            "name": "rel_ops_ext_sub",
            "extends": "rel_ops_ext_base",
            "fields": []
        }),
    )
    .await
    .map_err(|e| anyhow::anyhow!("subtype schema: {e}"))?;
    Ok(())
}

fn get(node_id: &str, name: &str, direction: &str) -> rel_ops::GetRelatedInput {
    rel_ops::GetRelatedInput {
        node_id: node_id.to_string(),
        relationship_name: name.to_string(),
        direction: direction.to_string(),
    }
}

/// Classification alone: a forward name declared only on the ancestor must
/// resolve as `Forward` for a subtype instance, not fall through to
/// `InvalidParams`.
#[tokio::test]
async fn inherited_forward_name_resolves_as_forward_not_invalid_params() -> Result<()> {
    let (svc, _t) = create_test_service().await?;
    create_base_and_subtype(&svc).await?;
    make_node(&svc, "sub1", "rel_ops_ext_sub").await?;

    let resolved =
        rel_ops::resolve_relationship_name(&svc, "sub1", "rel_ops_ext_sub", "story").await?;
    assert_eq!(resolved, ResolvedRelName::Forward);
    Ok(())
}

/// The actual bug: classification alone isn't enough -- a real edge attached
/// to a subtype instance (via the already chain-aware write path) must be
/// readable back through `get_related_nodes`, not just resolve without
/// erroring. Before the fix this returned `Err(InvalidParams)` here, which
/// `GraphResolver::fetch_related_nodes` turns into a silent empty result.
#[tokio::test]
async fn real_edge_on_subtype_instance_is_readable_through_inherited_forward_name() -> Result<()> {
    let (svc, _t) = create_test_service().await?;
    create_base_and_subtype(&svc).await?;
    make_node(&svc, "target1", "rel_ops_ext_target").await?;
    make_node(&svc, "sub1", "rel_ops_ext_sub").await?;

    // The write path is already extends-chain aware -- this succeeds today
    // even though `story` is declared only on the ancestor schema.
    svc.create_relationship("sub1", "story", "target1", json!({}))
        .await
        .expect("create_relationship must succeed for an inherited relationship");

    let out = rel_ops::get_related_nodes(&svc, get("sub1", "story", "out")).await?;
    assert_eq!(
        out.count, 1,
        "a real edge on a subtype instance must be readable through an \
         inherited forward name, not silently empty"
    );
    assert_eq!(out.related_nodes[0]["id"], "target1");
    assert_eq!(out.relationship_name, "story");
    assert_eq!(out.direction, "out");
    Ok(())
}

/// An undeclared name on a subtype must still error -- the fix must not
/// make the forward check accept anything not actually in the chain-merged
/// relationship set.
#[tokio::test]
async fn undeclared_name_on_subtype_still_errors() -> Result<()> {
    let (svc, _t) = create_test_service().await?;
    create_base_and_subtype(&svc).await?;
    make_node(&svc, "sub1", "rel_ops_ext_sub").await?;

    let err = rel_ops::get_related_nodes(&svc, get("sub1", "not_a_real_name", "out"))
        .await
        .expect_err("an undeclared name must still error, extends chain or not");
    assert!(matches!(
        err,
        nodespace_core::ops::OpsError::InvalidParams(_)
    ));
    Ok(())
}
