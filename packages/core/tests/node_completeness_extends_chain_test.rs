//! `check_node_completeness`, against a required relationship declared only
//! on an ancestor schema and inherited (not redeclared) by a subtype.
//!
//! `check_node_completeness` used to resolve a node's required relationships
//! via a direct own-schema lookup (`get_schema_node(&node.node_type)`),
//! iterating `schema.relationships` for entries with `required == Some(true)`.
//! That only sees the type's own directly-declared relationships, not the
//! ADR-078 `extends`-chain-merged set. A relationship declared `required:
//! true` only on an ancestor schema and inherited by a subtype was invisible
//! to this scan, so a subtype instance genuinely missing that inherited
//! required relationship was still reported `is_complete: true` with an
//! empty `missing_relationships` list -- an incorrect, silently-wrong
//! completeness result.
//!
//! Same fix pattern as `workflow_state.rs`, `validation.rs`,
//! `graph_resolver.rs`'s `is_declared_many_relationship`, and `rel_ops.rs`'s
//! `resolve_relationship_name`/`get_node_relationships`: resolve via
//! `NodeService::resolve_relationships` (the extends-chain-merged set)
//! instead of a direct own-schema lookup.

use anyhow::Result;
use nodespace_core::{
    db::SqliteStore, models::Node, schema::handle_create_schema, services::NodeService,
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

/// Base type declares `assignee` as `required: true`, out to
/// `completeness_ext_target`; the subtype extends the base without
/// redeclaring the relationship.
async fn create_base_and_subtype(svc: &Arc<NodeService>) -> Result<()> {
    handle_create_schema(
        svc,
        json!({
            "name": "completeness_ext_target",
            "fields": [{ "name": "title", "type": "string", "protection": "user", "indexed": false }]
        }),
    )
    .await
    .map_err(|e| anyhow::anyhow!("target schema: {e}"))?;

    handle_create_schema(
        svc,
        json!({
            "name": "completeness_ext_base",
            "fields": [],
            "relationships": [{
                "name": "assignee",
                "targetType": "completeness_ext_target",
                "direction": "out",
                "cardinality": "one",
                "required": true,
                "reverseName": "assigned_items",
                "reverseCardinality": "many"
            }]
        }),
    )
    .await
    .map_err(|e| anyhow::anyhow!("base schema: {e}"))?;

    handle_create_schema(
        svc,
        json!({
            "name": "completeness_ext_sub",
            "extends": "completeness_ext_base",
            "fields": []
        }),
    )
    .await
    .map_err(|e| anyhow::anyhow!("subtype schema: {e}"))?;
    Ok(())
}

/// The actual bug: a subtype instance with no edge attached for an inherited
/// `required: true` relationship must be reported incomplete, not silently
/// `is_complete: true`.
#[tokio::test]
async fn inherited_required_relationship_missing_reports_incomplete() -> Result<()> {
    let (svc, _t) = create_test_service().await?;
    create_base_and_subtype(&svc).await?;
    make_node(&svc, "sub1", "completeness_ext_sub").await?;

    let result = svc.check_node_completeness("sub1").await?;

    assert!(!result.is_complete);
    assert_eq!(result.missing_relationships, vec!["assignee".to_string()]);
    Ok(())
}

/// Once a real edge satisfying the inherited required relationship is
/// attached, the same subtype instance must be reported complete.
#[tokio::test]
async fn inherited_required_relationship_satisfied_reports_complete() -> Result<()> {
    let (svc, _t) = create_test_service().await?;
    create_base_and_subtype(&svc).await?;
    make_node(&svc, "target1", "completeness_ext_target").await?;
    make_node(&svc, "sub1", "completeness_ext_sub").await?;

    // The write path is already extends-chain aware -- this succeeds today
    // even though `assignee` is declared only on the ancestor schema.
    svc.create_relationship("sub1", "assignee", "target1", json!({}))
        .await?;

    let result = svc.check_node_completeness("sub1").await?;

    assert!(result.is_complete);
    assert!(result.missing_relationships.is_empty());
    Ok(())
}

/// `adr` declares `superseded_by` as a required `in`-direction relationship —
/// the target's view of its own forward `supersedes` edge (a self-referential
/// pair, so both ends exist when the schema is created). The stored edge is
/// `supersedes`, with the newer ADR as source and the older one as target.
async fn create_inbound_required_schema(svc: &Arc<NodeService>) -> Result<()> {
    handle_create_schema(
        svc,
        json!({
            "name": "completeness_in_adr",
            "fields": [],
            "relationships": [
                {
                    "name": "supersedes",
                    "targetType": "completeness_in_adr",
                    "direction": "out",
                    "cardinality": "one",
                    "reverseName": "superseded_by",
                    "reverseCardinality": "one"
                },
                {
                    "name": "superseded_by",
                    "targetType": "completeness_in_adr",
                    "direction": "in",
                    "cardinality": "one",
                    "required": true,
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

/// A required `in`-direction relationship with no inbound edge is missing.
#[tokio::test]
async fn required_inbound_relationship_missing_reports_incomplete() -> Result<()> {
    let (svc, _t) = create_test_service().await?;
    create_inbound_required_schema(&svc).await?;
    make_node(&svc, "old", "completeness_in_adr").await?;

    let result = svc.check_node_completeness("old").await?;

    assert!(!result.is_complete);
    assert_eq!(
        result.missing_relationships,
        vec!["superseded_by".to_string()]
    );
    Ok(())
}

/// A real edge attached from the other side satisfies the required
/// `in`-direction relationship. The check used to look for an outbound
/// `superseded_by` edge from the node — which is never stored — so the node
/// was reported incomplete forever.
#[tokio::test]
async fn required_inbound_relationship_satisfied_from_other_side_reports_complete() -> Result<()> {
    let (svc, _t) = create_test_service().await?;
    create_inbound_required_schema(&svc).await?;
    make_node(&svc, "old", "completeness_in_adr").await?;
    make_node(&svc, "new", "completeness_in_adr").await?;

    svc.create_relationship("new", "supersedes", "old", json!({}))
        .await?;

    let result = svc.check_node_completeness("old").await?;

    assert!(
        result.is_complete,
        "missing: {:?}",
        result.missing_relationships
    );
    assert!(result.missing_relationships.is_empty());

    // The edge's source end is not satisfied by it: `new` has no inbound
    // `supersedes` edge of its own.
    let source = svc.check_node_completeness("new").await?;
    assert_eq!(
        source.missing_relationships,
        vec!["superseded_by".to_string()]
    );
    Ok(())
}

/// The write path also accepts an edge written through the `in` name itself
/// (stored under `superseded_by` from this node's own end). That shape must
/// satisfy the required relationship too.
#[tokio::test]
async fn required_inbound_relationship_written_through_in_name_reports_complete() -> Result<()> {
    let (svc, _t) = create_test_service().await?;
    create_inbound_required_schema(&svc).await?;
    make_node(&svc, "old", "completeness_in_adr").await?;
    make_node(&svc, "new", "completeness_in_adr").await?;

    svc.create_relationship("old", "superseded_by", "new", json!({}))
        .await?;

    let result = svc.check_node_completeness("old").await?;

    assert!(
        result.is_complete,
        "missing: {:?}",
        result.missing_relationships
    );
    Ok(())
}

/// Another schema declaring the same forward name toward the same type shares
/// the stored `relationship_type`. Its edge must not satisfy an `in`
/// declaration whose `targetType` names a different source type — but an
/// ADR-078 subtype of the declared source type does.
#[tokio::test]
async fn required_inbound_relationship_narrows_by_source_type() -> Result<()> {
    let (svc, _t) = create_test_service().await?;
    create_inbound_required_schema(&svc).await?;
    handle_create_schema(
        &svc,
        json!({
            "name": "completeness_in_memo",
            "fields": [],
            "relationships": [{
                "name": "supersedes",
                "targetType": "completeness_in_adr",
                "direction": "out",
                "cardinality": "one",
                "reverseName": "superseded_by_memo",
                "reverseCardinality": "one"
            }]
        }),
    )
    .await
    .map_err(|e| anyhow::anyhow!("memo schema: {e}"))?;
    handle_create_schema(
        &svc,
        json!({
            "name": "completeness_in_adr_sub",
            "extends": "completeness_in_adr",
            "fields": []
        }),
    )
    .await
    .map_err(|e| anyhow::anyhow!("adr subtype schema: {e}"))?;

    make_node(&svc, "old", "completeness_in_adr").await?;
    make_node(&svc, "memo1", "completeness_in_memo").await?;
    svc.create_relationship("memo1", "supersedes", "old", json!({}))
        .await?;

    let result = svc.check_node_completeness("old").await?;
    assert_eq!(
        result.missing_relationships,
        vec!["superseded_by".to_string()],
        "a memo's `supersedes` edge is not an adr superseding this one"
    );

    make_node(&svc, "sub1", "completeness_in_adr_sub").await?;
    svc.create_relationship("sub1", "supersedes", "old", json!({}))
        .await?;

    let result = svc.check_node_completeness("old").await?;
    assert!(
        result.is_complete,
        "an adr subtype source satisfies it; missing: {:?}",
        result.missing_relationships
    );
    Ok(())
}
